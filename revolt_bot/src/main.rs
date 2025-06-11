//! A fully-featured Revolt bot example demonstrating CLI parsing, env-file loading,
//! authentication (username/password or token), JSON config validation and usage,
//! message handling with concurrency-safe state, robust channel name tracking, and
//! verbose & admin dashboard logging.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rand::Rng;
use regex::Regex;
use revolt_api::{
    api::messages::FetchMessagesOptions,
    client::*,
    event_handler::*,
    types::{bulk_message_response::BulkMessageResponse, message::Message, user::User},
};
use serde::Deserialize;
use std::{
    collections::{HashMap, HashSet},
    env, fs, process,
    sync::Arc,
    time::Duration,
};
use tokio::{
    sync::{watch, Mutex},
    task::JoinHandle,
    time::{sleep, Instant},
};
use ulid::Ulid;

#[derive(Debug, Deserialize, Clone)]
struct AdminConfig {
    channel: String,
    start: String,
    stop: String,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "snake_case")]
struct Reply {
    #[serde(rename = "type")]
    r#type: String,
    content: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
struct ClaimConfig {
    bot_id: String,
    startswith: String,
    reply: Reply,
}

#[derive(Debug, Deserialize, Clone)]
struct BotConfig {
    admin: Vec<AdminConfig>,
    claim: Vec<ClaimConfig>,
}

#[derive(Debug)]
struct ClaimTimingData {
    trigger_message_id: String,
    trigger_arrival_instant: Instant,
    collected_messages: Vec<(Instant, DateTime<Utc>, Message)>,
    bot_reply_message_id: Option<String>,
    timeout_task_handle: Option<JoinHandle<()>>,
}

#[derive(Clone)]
struct MyHandler {
    config: Arc<BotConfig>,
    is_running: Arc<Mutex<bool>>,
    verbose: bool,
    admin_log: bool,
    /// Watch transmitter for sleep timeout (None means no auto-stop timer)
    sleep_tx: Arc<Mutex<watch::Sender<Option<Duration>>>>,
    channels_map: Arc<Mutex<HashMap<String, String>>>,
    server_map: Arc<Mutex<HashMap<String, String>>>,
    server_names: Arc<Mutex<HashMap<String, String>>>,
    bot_user_id: Arc<Mutex<Option<String>>>,
    /// Map tracking active claims for websocket timing
    active_claims: Arc<Mutex<HashMap<String, ClaimTimingData>>>,
}

impl MyHandler {
    /// Attempt to insert or update a channel name in `channels_map`.
    /// If `channel_name` is None, we treat it as an empty string (or skip).
    async fn set_channel_name(&self, channel_id: &str, channel_name: Option<&String>) {
        let mut map = self.channels_map.lock().await;
        let name = channel_name.cloned().unwrap_or_default();
        map.insert(channel_id.to_string(), name);
    }

    /// Remove channel from `channels_map` if it exists.
    async fn remove_channel_name(&self, channel_id: &str) {
        let mut map = self.channels_map.lock().await;
        map.remove(channel_id);
    }

    /// Fetch the channel name by ID from `channels_map`, if known.
    async fn get_channel_name(&self, channel_id: &str) -> Option<String> {
        let map = self.channels_map.lock().await;
        map.get(channel_id).cloned()
    }

    /// Set/update the server association for a channel
    async fn set_channel_server(&self, channel_id: &str, server_id: &str) {
        let mut map = self.server_map.lock().await;
        map.insert(channel_id.to_string(), server_id.to_string());
    }

    /// Remove channel-server association when a channel is deleted
    async fn remove_channel_server(&self, channel_id: &str) {
        let mut map = self.server_map.lock().await;
        map.remove(channel_id);
    }

    /// Get the server ID for a channel, if it exists
    async fn get_channel_server(&self, channel_id: &str) -> Option<String> {
        let map = self.server_map.lock().await;
        map.get(channel_id).cloned()
    }

    /// Set/update a server's name
    async fn set_server_name(&self, server_id: &str, name: &str) {
        let mut map = self.server_names.lock().await;
        map.insert(server_id.to_string(), name.to_string());
    }

    /// Get a server's name, if known
    async fn get_server_name(&self, server_id: &str) -> Option<String> {
        let map = self.server_names.lock().await;
        map.get(server_id).cloned()
    }

    /// Helper to log to console and admin channels.
    async fn log_to_admin_and_console(&self, client: &RevoltClient, message: &str) {
        // Always log to console:
        eprintln!("[ADMIN-LOG] {}", message);

        // If admin_log is enabled, send to all admin channels
        if self.admin_log {
            for admin in &self.config.admin {
                if let Err(e) = client
                    .send_message(&admin.channel, message, None, None, None)
                    .await
                {
                    eprintln!(
                        "[ERROR] Failed to send admin log to channel {}: {:?}",
                        admin.channel, e
                    );
                }
            }
        }
    }
}

/// Helper function to process claim results based on collected WS messages and fetched Nonce messages
async fn process_claim_results(
    handler: MyHandler,
    client: RevoltClient,
    claim_data: ClaimTimingData,
    users_map_from_fetch: HashMap<String, String>, // Pass users map needed for nonce calc
    nonce_trigger_ulid: Option<Ulid>,              // Pass trigger nonce if available
    nonce_reply_messages: Vec<Message>,            // Pass fetched replies for nonce calc
) {
    if handler.verbose {
        println!(
            "[VERBOSE] Processing claim results for trigger {}",
            claim_data.trigger_message_id
        );
    }

    // --- WebSocket Timing Calculation ---
    let mut ws_timed_replies: Vec<(Instant, DateTime<Utc>, &Message)> = claim_data
        .collected_messages
        .iter()
        .skip(1) // Skip trigger message
        .map(|(instant, time, msg)| (*instant, *time, msg))
        .collect();

    ws_timed_replies.sort_by_key(|(instant, _, _)| *instant);

    let mut ws_results: HashMap<String, (i64, i64)> = HashMap::new(); // UserID -> (TimeVsWinnerMs, TimeVsTriggerMs)
                                                                      // Get the winner's Instant and DateTime<Utc>
    let ws_winner_opt = ws_timed_replies.first();

    // Find our message in the collected list to determine if we won based on WS time
    let ws_we_won = ws_winner_opt
        .is_some_and(|(_, _, msg)| claim_data.bot_reply_message_id.as_ref() == Some(&msg.id));

    // Calculate results only if there is a winner
    if let Some((winner_instant, _winner_time, _winner_msg)) = ws_winner_opt {
        for (arrival_instant, _arrival_time, msg) in &ws_timed_replies {
            let time_vs_trigger = arrival_instant
                .duration_since(claim_data.trigger_arrival_instant)
                .as_millis() as i64;
            // Use the winner_instant obtained from ws_winner_opt
            let time_vs_winner = arrival_instant.duration_since(*winner_instant).as_millis() as i64;
            ws_results
                .entry(msg.author.clone())
                .or_insert((time_vs_winner, time_vs_trigger));
        }
    }

    // --- Nonce Timing Calculation ---
    let mut nonce_results: HashMap<String, (i64, i64)> = HashMap::new(); // UserID -> (TimeVsWinnerMs, TimeVsTriggerMs)
    let mut nonce_timed_replies: Vec<(DateTime<Utc>, &Message)> = Vec::new();
    let mut nonce_we_won = false;

    if let Some(trigger_ulid) = nonce_trigger_ulid {
        let trigger_nonce_time: DateTime<Utc> = trigger_ulid.datetime().into();

        for msg in nonce_reply_messages.iter() {
            if let Some(nonce_str) = &msg.nonce {
                if let Ok(ulid) = Ulid::from_string(nonce_str) {
                    nonce_timed_replies.push((ulid.datetime().into(), msg));
                }
            }
        }
        nonce_timed_replies.sort_by_key(|(time, _)| *time);

        if let Some((winner_nonce_time, winner_msg)) = nonce_timed_replies.first() {
            nonce_we_won = claim_data.bot_reply_message_id.as_ref() == Some(&winner_msg.id);

            for (reply_nonce_time, msg) in &nonce_timed_replies {
                let time_vs_trigger = reply_nonce_time
                    .signed_duration_since(trigger_nonce_time)
                    .num_milliseconds();
                let time_vs_winner = reply_nonce_time
                    .signed_duration_since(*winner_nonce_time)
                    .num_milliseconds();
                nonce_results
                    .entry(msg.author.clone())
                    .or_insert((time_vs_winner, time_vs_trigger));
            }
        }
    } else if handler.verbose {
        println!(
            "[VERBOSE] Nonce calculation skipped: Trigger message {} had no valid nonce.",
            claim_data.trigger_message_id
        );
    }

    // --- Combine and Format ---
    let bot_user_id_opt = handler.bot_user_id.lock().await.clone();
    let bot_user_id = bot_user_id_opt.as_deref().unwrap_or(""); // Use empty string if ID not set

    // Get channel ID from any message in the WS list (prefer winner)
    let channel_id = ws_timed_replies.first().map_or_else(
        || {
            // Fallback: get from trigger message if ws_timed_replies is empty
            claim_data
                .collected_messages
                .first()
                .map_or("unknown", |(_, _, m)| &m.channel)
        },
        |(_, _, m)| &m.channel,
    );

    // Base log message with server/channel link
    let server_id = handler.get_channel_server(channel_id).await;
    let base_log = if let Some(sid) = server_id {
        let server_name = handler
            .get_server_name(&sid)
            .await
            .unwrap_or_else(|| "Unknown Server".to_string());
        let link = format!(
            "https://revolt.onech.at/server/{}/channel/{}",
            sid, channel_id
        );
        format!("🔗 **Server:** [{}]({})", server_name, link)
    } else {
        let link = format!("https://revolt.onech.at/channel/{}", channel_id);
        format!("🔗 **Channel:** [Direct Link]({})", link)
    };

    let mut final_log_message = String::new();

    // Determine overall winner based on WebSocket time
    if let Some((_, _, winner_msg)) = ws_timed_replies.first() {
        let winner_id = &winner_msg.author;
        let winner_name = users_map_from_fetch
            .get(winner_id)
            .cloned()
            .unwrap_or_else(|| format!("UnknownUser ({})", winner_id));

        // Get winner's times (WS winner)
        let (_, ws_winner_vs_trigger) =
            ws_results.get(winner_id).cloned().unwrap_or((0, 0));
        // Get winner's nonce time vs trigger (might be different from WS winner)
        let (_nonce_winner_vs_winner, nonce_winner_vs_trigger) =
            nonce_results.get(winner_id).cloned().unwrap_or((0, 0));

        if ws_we_won {
            final_log_message.push_str(&format!(
                "✨🏆 **Claim Won!** (WS Time) 🏆✨\n\n{}\n\n",
                base_log
            ));
            // Show our WS time vs trigger and our Nonce time vs trigger
            final_log_message.push_str(&format!(
                "⏱️ Response Time: {} ms (nonce: {} ms)\n",
                ws_winner_vs_trigger,
                nonce_winner_vs_trigger // Use our own times here
            ));
        } else {
            final_log_message.push_str(&format!(
                "💥 **Claim Lost!** (WS Time) 💥\n\n{}\n\n",
                base_log
            ));
            // Show WS winner's name, their WS time vs trigger, and their Nonce time vs trigger
            final_log_message.push_str(&format!(
                "🏆 Winner: {} ({} ms) (nonce: {} ms)\n",
                winner_name, ws_winner_vs_trigger, nonce_winner_vs_trigger
            ));
        }
        final_log_message.push_str("--------------------\n");

        // Build Leaderboard (sorted by WS time)
        let mut seen = HashSet::new();
        for (_, _ws_arrival_time, participant_msg) in &ws_timed_replies {
            let p_id = &participant_msg.author;

            if !seen.insert(p_id) {
                continue;
            }

            let mut p_name = users_map_from_fetch
                .get(p_id)
                .cloned()
                .unwrap_or_else(|| format!("UnknownUser ({})", p_id));

            if p_id == bot_user_id {
                p_name = format!("**{}**", p_name); // Bold our bot
            }

            let (ws_vs_winner, ws_vs_trigger) = ws_results.get(p_id).cloned().unwrap_or((0, 0));
            // Get participant's nonce time vs trigger
            let (_nonce_vs_winner, nonce_vs_trigger) =
                nonce_results.get(p_id).cloned().unwrap_or((0, 0));

            let entry = if participant_msg.id == winner_msg.id {
                format!(
                    "{}: Winner ({} ms) (nonce: {} ms)\n",
                    p_name, ws_vs_trigger, nonce_vs_trigger
                )
            } else {
                format!(
                    "{}: +{} ms (Total: {} ms, nonce: {} ms)\n",
                    p_name, ws_vs_winner, ws_vs_trigger, nonce_vs_trigger
                )
            };
            final_log_message.push_str(&entry);
        }
    } else {
        // Handle case where no WS replies were received (only trigger)
        // Check if nonce calculation yielded a winner
        if let Some((_nonce_time, nonce_winner_msg)) = nonce_timed_replies.first() {
            let winner_id = &nonce_winner_msg.author;
            let winner_name = users_map_from_fetch
                .get(winner_id)
                .cloned()
                .unwrap_or_else(|| format!("UnknownUser ({})", winner_id));
            let (_nonce_vs_winner, nonce_vs_trigger) =
                nonce_results.get(winner_id).cloned().unwrap_or((0, 0));

            if nonce_we_won {
                final_log_message.push_str(&format!(
                    "✨🏆 **Claim Won!** (Nonce Time Only) 🏆✨\n\n{}\n\n",
                    base_log
                ));
                final_log_message.push_str(&format!(
                    "⏱️ Response Time: (No WS Data) (nonce: {} ms)\n",
                    nonce_vs_trigger
                ));
            } else {
                final_log_message.push_str(&format!(
                    "💥 **Claim Lost!** (Nonce Time Only) 💥\n\n{}\n\n",
                    base_log
                ));
                final_log_message.push_str(&format!(
                    "🏆 Winner: {} (No WS Data) (nonce: {} ms)\n",
                    winner_name, nonce_vs_trigger
                ));
            }
            final_log_message.push_str("--------------------\n");
            // Build Leaderboard based on Nonce
            for (_nonce_time, participant_msg) in &nonce_timed_replies {
                let p_id = &participant_msg.author;
                let mut p_name = users_map_from_fetch
                    .get(p_id)
                    .cloned()
                    .unwrap_or_else(|| format!("UnknownUser ({})", p_id));

                if p_id == bot_user_id {
                    p_name = format!("**{}**", p_name); // Bold our bot
                }
                let (nonce_vs_winner, nonce_vs_trigger) =
                    nonce_results.get(p_id).cloned().unwrap_or((0, 0));

                // Check if the current participant's message ID matches the nonce winner's message ID
                let entry = if participant_msg.id == nonce_winner_msg.id {
                    // Nonce Winner entry
                    format!(
                        "{}: Winner (No WS Data) (nonce: {} ms)\n",
                        p_name, nonce_vs_trigger
                    )
                } else {
                    // Other entries
                    format!(
                        "{}: (No WS Data) (nonce: +{} ms, Total: {} ms)\n",
                        p_name, nonce_vs_winner, nonce_vs_trigger
                    )
                };
                final_log_message.push_str(&entry);
            }
        } else {
            // No WS replies and no Nonce replies
            final_log_message = format!(
                "{}\n\n🤷 No valid replies found (WS or Nonce) after trigger.",
                base_log
            );
        }
    }

    // Log to admin channels and console
    handler
        .log_to_admin_and_console(&client, &final_log_message)
        .await;
}

/// ---------------------------------------------------------------------
/// Implementation of the Revolt EventHandler
/// ---------------------------------------------------------------------
#[async_trait]
impl EventHandler for MyHandler {
    async fn on_message(&self, client: &RevoltClient, msg: &Message) {
        let ws_arrival_instant = Instant::now();
        let current_arrival_time = Utc::now();

        // --- Check if this message belongs to an active claim being timed ---
        let mut claim_completed_data: Option<ClaimTimingData> = None;
        {
            // Scope for active_claims lock
            let mut active_claims_map = self.active_claims.lock().await;
            if let Some(claim_data) = active_claims_map.get_mut(&msg.channel) {
                // Record this message
                claim_data.collected_messages.push((
                    ws_arrival_instant,
                    current_arrival_time,
                    msg.clone(),
                ));
                if self.verbose {
                    println!(
                        "[VERBOSE] Collected message {} for active claim in channel {}",
                        msg.id, msg.channel
                    );
                }

                // Check if it's our reply message
                if let Some(bot_reply_id) = &claim_data.bot_reply_message_id {
                    if msg.id == *bot_reply_id {
                        if self.verbose {
                            println!(
                                "[VERBOSE] Bot reply {} received. Finalizing claim for channel {}.",
                                msg.id, msg.channel
                            );
                        }
                        // Claim finished! Cancel timeout, remove from map, prepare for processing
                        if let Some(handle) = claim_data.timeout_task_handle.take() {
                            handle.abort(); // Cancel the timeout task
                        }
                        // Remove from map and store data to process outside the lock
                        claim_completed_data = active_claims_map.remove(&msg.channel);
                    }
                }
            }
        } // Lock released here

        // --- Process completed claim (if any) ---
        if let Some(data_to_process) = claim_completed_data {
            // Fetch messages *after* trigger for nonce calculation
            let trigger_ulid = data_to_process
                .collected_messages
                .first()
                .and_then(|(_, _, trigger_msg)| trigger_msg.nonce.as_ref())
                .and_then(|n| Ulid::from_string(n).ok());

            let fetch_replies_opts = FetchMessagesOptions {
                limit: Some(20), // Fetch a bit more for safety
                sort: Some("Oldest".to_string()),
                include_users: Some(true),
                after: Some(data_to_process.trigger_message_id.clone()),
                ..Default::default()
            };

            let (nonce_reply_messages, users_map_from_fetch) = match client
                .fetch_messages(
                    &data_to_process.collected_messages[0].2.channel,
                    Some(fetch_replies_opts),
                ) // Use channel from trigger
                .await
            {
                Ok(reply_response) => match reply_response {
                    BulkMessageResponse::MessagesWithUsers {
                        messages, users, ..
                    } => {
                        let user_map: HashMap<String, String> =
                            users.into_iter().map(|u| (u.id, u.username)).collect();
                        (messages, user_map)
                    }
                    _ => {
                        eprintln!("[ERROR] Fetched replies for nonce calc but user data missing for trigger {}.", data_to_process.trigger_message_id);
                        (Vec::new(), HashMap::new()) // Proceed with empty data
                    }
                },
                Err(e) => {
                    eprintln!(
                        "[ERROR] Failed to fetch replies for nonce calc (trigger {}): {:?}",
                        data_to_process.trigger_message_id, e
                    );
                    (Vec::new(), HashMap::new()) // Proceed with empty data
                }
            };

            // Spawn the processing task
            tokio::spawn(process_claim_results(
                self.clone(),
                client.clone(),
                data_to_process,
                users_map_from_fetch,
                trigger_ulid,
                nonce_reply_messages,
            ));
            return; // Don't process this message further (it was part of the claim)
        }

        // --- Regular message processing starts here (Admin, Triggers, etc.) ---

        // Ignore messages without content
        let Some(content) = &msg.content else {
            return;
        };

        // --- Admin Command Handling ---
        for admin_section in &self.config.admin {
            if admin_section.channel == msg.channel {
                // (Admin command logic remains unchanged - omitted for brevity)
                let tokens: Vec<&str> = content.split_whitespace().collect();
                if tokens.is_empty() {
                    continue;
                }
                if tokens[0] == admin_section.start {
                    // ... start logic ...
                    let duration_opt = if tokens.len() > 1 {
                        match tokens[1].parse::<u64>() {
                            Ok(minutes) if minutes > 0 => Some(Duration::from_secs(minutes * 60)),
                            _ => {
                                let err_msg =
                                    format!(":x: Failure: Invalid duration `{}`.", tokens[1]);
                                if let Err(e) = msg.reply(&err_msg).await {
                                    eprintln!(
                                        "[ERROR] Failed to send invalid duration message: {:?}",
                                        e
                                    );
                                }
                                return;
                            }
                        }
                    } else {
                        None
                    };
                    *self.is_running.lock().await = true;
                    if self.sleep_tx.lock().await.send(duration_opt).is_err() {
                        eprintln!("[ERROR] Failed to update sleep duration.");
                    }
                    if self.admin_log {
                        let log_msg = if let Some(duration) = duration_opt {
                            format!(
                                ":rocket: Bot started for {} minute(s).",
                                duration.as_secs() / 60
                            )
                        } else {
                            ":rocket: Bot started indefinitely.".to_string()
                        };
                        if let Err(e) = msg.reply(&log_msg).await {
                            eprintln!("[ERROR] Failed to send admin log for .start: {:?}", e);
                        }
                    }
                    return; // Admin command processed
                } else if tokens[0] == admin_section.stop {
                    // ... stop logic ...
                    *self.is_running.lock().await = false;
                    if self.sleep_tx.lock().await.send(None).is_err() {
                        eprintln!("[ERROR] Failed to clear sleep duration.");
                    }
                    if self.admin_log {
                        if let Err(e) = msg.reply(":red_circle: Bot stopped.").await {
                            eprintln!("[ERROR] Failed to send admin log for .stop: {:?}", e);
                        }
                    }
                    return; // Admin command processed
                }
            }
        }

        // --- Claim Trigger Handling ---

        // Only process claims if the bot is running.
        let is_running_now = *self.is_running.lock().await;
        if !is_running_now {
            // Don't log here if verbose, might be noisy
            return;
        }

        // Check if channel already has an active claim
        if self.active_claims.lock().await.contains_key(&msg.channel) {
            if self.verbose {
                println!(
                    "[VERBOSE] Ignoring trigger in channel {} as a claim is already active.",
                    msg.channel
                );
            }
            return; // Don't start a new claim if one is running
        }

        let re = Regex::new(r"(\d+)").unwrap();
        for claim_section in &self.config.claim {
            // Check if it's a trigger message
            if claim_section.bot_id == msg.author
                && content
                    .to_lowercase()
                    .starts_with(&claim_section.startswith.to_lowercase())
            {
                if self.verbose {
                    println!(
                        "[VERBOSE] Claim trigger detected in channel {}: {}",
                        msg.channel, msg.id
                    );
                }

                let mut reply_text_opt: Option<String> = None;

                // --- Generate Reply Text (logic unchanged) ---
                match claim_section.reply.r#type.as_str() {
                    "fixed" => {
                        if let Some(reply_text) = &claim_section.reply.content {
                            reply_text_opt = Some(reply_text.clone());
                        } else {
                            eprintln!("[WARN] 'fixed' reply type found but 'content' was missing.");
                        }
                    }
                    "random_letter" => {
                        let reply_text = {
                            let mut rng = rand::rng();
                            let letter = (b'a' + rng.random_range(0..26)) as char;
                            letter.to_string()
                        };

                        reply_text_opt = Some(reply_text);
                    }
                    "channel_number" => {
                        if let Some(name) = self.get_channel_name(&msg.channel).await {
                            if let Some(captures) = re.captures(&name) {
                                reply_text_opt = Some(captures[1].to_string());
                            } else {
                                self.log_to_admin_and_console(
                                    client,
                                    &format!("[WARN] No number found in channel name: '{}'.", name),
                                )
                                .await;
                            }
                        } else {
                            self.log_to_admin_and_console(
                                client,
                                &format!("[WARN] Channel name not found for ID {}.", msg.channel),
                            )
                            .await;
                        }
                    }
                    other => {
                        eprintln!("[WARN] Unknown reply type '{}'.", other);
                    }
                }

                // --- Send Reply & Start WS Timing ---
                if let Some(reply_text) = reply_text_opt {
                    // Use a standard ULID nonce for the reply
                    let reply_nonce = Ulid::new().to_string();
                    if self.verbose {
                        println!(
                            "[VERBOSE] Sending reply '{}' with nonce {}",
                            reply_text, reply_nonce
                        );
                    }

                    match msg
                        .send_message(&reply_text, Some(false), Some(reply_nonce))
                        .await
                    {
                        Ok(sent_msg) => {
                            let bot_reply_id = sent_msg.id.clone();
                            if self.verbose {
                                println!(
                                    "[VERBOSE] Reply sent: {}. Starting WS timing.",
                                    bot_reply_id
                                );
                            }

                            // Create initial timing data
                            let mut claim_data = ClaimTimingData {
                                trigger_message_id: msg.id.clone(),
                                trigger_arrival_instant: ws_arrival_instant,
                                collected_messages: vec![(
                                    ws_arrival_instant,
                                    current_arrival_time,
                                    msg.clone(),
                                )], // Start with trigger msg
                                bot_reply_message_id: Some(bot_reply_id.clone()),
                                timeout_task_handle: None, // Will be set below
                            };

                            // Spawn timeout task
                            let timeout_duration = Duration::from_secs(10);
                            let active_claims_clone = self.active_claims.clone();
                            let channel_id_clone = msg.channel.clone();
                            let trigger_id_clone = msg.id.clone();
                            let handler_clone = self.clone(); // Clone handler for verbose logging in timeout
                            let client_clone = client.clone(); // Clone client for potential fetch in timeout processing

                            let timeout_handle = tokio::spawn(async move {
                                sleep(timeout_duration).await;
                                // Timeout elapsed, try to remove the claim data
                                let mut timed_out_data: Option<ClaimTimingData> = None;
                                {
                                    // Lock scope
                                    let mut active_claims_map = active_claims_clone.lock().await;
                                    // Check if the claim still exists (it might have completed normally)
                                    if let Some(claim_entry) =
                                        active_claims_map.get(&channel_id_clone)
                                    {
                                        // Verify it's the same trigger instance, just in case
                                        if claim_entry.trigger_message_id == trigger_id_clone {
                                            if handler_clone.verbose {
                                                println!("[VERBOSE] Claim timeout reached for channel {}. Removing and processing.", channel_id_clone);
                                            }
                                            timed_out_data =
                                                active_claims_map.remove(&channel_id_clone);
                                        }
                                    }
                                } // Lock released

                                // Process the timed-out data if we removed it
                                if let Some(data_to_process) = timed_out_data {
                                    // Fetch messages *after* trigger for nonce calculation (similar to normal completion)
                                    let trigger_ulid = data_to_process
                                        .collected_messages
                                        .first()
                                        .and_then(|(_, _, trigger_msg)| trigger_msg.nonce.as_ref())
                                        .and_then(|n| Ulid::from_string(n).ok());

                                    let fetch_replies_opts = FetchMessagesOptions {
                                        limit: Some(20),
                                        sort: Some("Oldest".to_string()),
                                        include_users: Some(true),
                                        after: Some(data_to_process.trigger_message_id.clone()),
                                        ..Default::default()
                                    };

                                    let (nonce_reply_messages, users_map_from_fetch) =
                                        match client_clone
                                            .fetch_messages(
                                                &data_to_process.collected_messages[0].2.channel,
                                                Some(fetch_replies_opts),
                                            )
                                            .await
                                        {
                                            Ok(reply_response) => match reply_response {
                                                BulkMessageResponse::MessagesWithUsers {
                                                    messages,
                                                    users,
                                                    ..
                                                } => {
                                                    let user_map: HashMap<String, String> = users
                                                        .into_iter()
                                                        .map(|u| (u.id, u.username))
                                                        .collect();
                                                    (messages, user_map)
                                                }
                                                _ => (Vec::new(), HashMap::new()),
                                            },
                                            Err(_) => (Vec::new(), HashMap::new()),
                                        };

                                    // Spawn the processing task for the timed-out data
                                    tokio::spawn(process_claim_results(
                                        handler_clone, // Use cloned handler
                                        client_clone,  // Use cloned client
                                        data_to_process,
                                        users_map_from_fetch,
                                        trigger_ulid,
                                        nonce_reply_messages,
                                    ));
                                } else if handler_clone.verbose {
                                    println!("[VERBOSE] Claim timeout task fired for channel {}, but claim was already completed or removed.", channel_id_clone);
                                }
                            });

                            // Store the handle and insert into the map
                            claim_data.timeout_task_handle = Some(timeout_handle);
                            {
                                // Lock scope
                                let mut active_claims_map = self.active_claims.lock().await;
                                active_claims_map.insert(msg.channel.clone(), claim_data);
                            } // Lock released
                        }
                        Err(e) => {
                            eprintln!(
                                "[ERROR] Failed to send reply '{}' in channel {}: {:?}",
                                reply_text, msg.channel, e
                            );
                            // Don't start timing if reply failed
                        }
                    }
                }

                // Once we match and attempt a claim trigger, stop checking further claim configs.
                break;
            }
        }
    }

    async fn on_ready(&self, _client: &RevoltClient, data: &ReadyEvent) {
        // Populate our server names map from data.servers
        for server_obj in &data.servers {
            if let (Some(id_val), Some(name_val)) = (server_obj.get("_id"), server_obj.get("name"))
            {
                if let (Some(server_id), Some(server_name)) = (id_val.as_str(), name_val.as_str()) {
                    self.set_server_name(server_id, server_name).await;

                    if self.verbose {
                        println!(
                            "[VERBOSE] Cached server {} with name: {}",
                            server_id, server_name
                        );
                    }
                }
            }
        }

        // Populate our local channel map from `data.channels`, which is a serde_json::Value
        // array. We need to parse out `_id` and `name` fields.
        let arr = &data.channels;
        for channel_obj in arr {
            if let (Some(id_val), Some(ctype_val)) =
                (channel_obj.get("_id"), channel_obj.get("channel_type"))
            {
                // Attempt to parse them as strings.
                if let (Some(channel_id), Some(channel_type)) =
                    (id_val.as_str(), ctype_val.as_str())
                {
                    // Not all channels have a 'name' field (e.g., DM channels).
                    let name = channel_obj.get("name").and_then(|n| n.as_str());
                    // Insert or skip based on existence of `name`.
                    let name_str = name.map(|s| s.to_string());

                    // For production, you might filter only text channels, etc.
                    // We'll store any channel that has a name; otherwise an empty string.
                    self.set_channel_name(channel_id, name_str.as_ref()).await;

                    // If this channel belongs to a server, track the server-channel relationship
                    if let Some(server_val) = channel_obj.get("server") {
                        if let Some(server_id) = server_val.as_str() {
                            self.set_channel_server(channel_id, server_id).await;

                            if self.verbose {
                                println!(
                                    "[VERBOSE] Channel {} belongs to server {}",
                                    channel_id, server_id
                                );
                            }
                        }
                    }

                    if self.verbose {
                        println!(
                            "[VERBOSE] Cached channel {} (type={}), name={:?}",
                            channel_id, channel_type, name_str
                        );
                    }
                }
            }
        }

        // Find and store our bot's user ID
        for user_value in &data.users {
            // Assuming the first user in the list is *us* (common but not guaranteed by API)
            // A more robust way might involve fetching `/users/@me` if needed,
            // but this usually works for the Ready event.
            match serde_json::from_value::<User>(user_value.clone()) {
                Ok(user) => {
                    println!("Found our user ID: {}", user.id);
                    let mut bot_id_lock = self.bot_user_id.lock().await;
                    *bot_id_lock = Some(user.id);
                    // Assuming we only care about the first user object which should be the bot's own.
                    break;
                }
                Err(e) => {
                    eprintln!(
                        "[ERROR] Failed to deserialize user from Ready event: {:?}\nValue: {:?}",
                        e, user_value
                    );
                }
            }
        }
        // Check if ID was set
        if self.bot_user_id.lock().await.is_none() {
            eprintln!("[WARN] Could not determine bot's own user ID from Ready event.");
        }
    }

    async fn on_channel_create(&self, _client: &RevoltClient, event: &ChannelCreateEvent) {
        println!(
            "Channel created: {} (type={:?}, name={:?})",
            event.id, event.channel_type, event.name
        );

        // Store or update the channel name in our map
        self.set_channel_name(&event.id, event.name.as_ref()).await;

        // If this is a server channel, track the server-channel relationship
        if let Some(server_id) = &event.server {
            self.set_channel_server(&event.id, server_id).await;
            if self.verbose {
                println!(
                    "[VERBOSE] New channel {} belongs to server {}",
                    event.id, server_id
                );
            }
        }
    }

    async fn on_channel_update(&self, _client: &RevoltClient, event: &ChannelUpdateEvent) {
        println!("Channel updated: {}, fields: {:?}", event.id, event.data);

        // The .data field holds only partial updates. If the name changed,
        // it should appear in event.data["name"]. We'll check for that.
        if let Some(name_val) = event.data.get("name") {
            if let Some(name_str) = name_val.as_str() {
                // Update the stored name
                self.set_channel_name(&event.id, Some(&name_str.to_string()))
                    .await;
            }
        }

        // Check if server association changed (rare but possible)
        if let Some(server_val) = event.data.get("server") {
            if let Some(server_id) = server_val.as_str() {
                self.set_channel_server(&event.id, server_id).await;
                if self.verbose {
                    println!(
                        "[VERBOSE] Channel {} now belongs to server {}",
                        event.id, server_id
                    );
                }
            }
        }
    }

    async fn on_channel_delete(&self, _client: &RevoltClient, event: &ChannelDeleteEvent) {
        println!("Channel deleted: {}", event.id);

        // Remove it from our maps
        self.remove_channel_name(&event.id).await;
        self.remove_channel_server(&event.id).await;
    }
}

/// The main entry point of the bot
#[tokio::main]
async fn main() {
    let args = env::args().skip(1).collect::<Vec<String>>();

    // Show help and exit if requested.
    if args.iter().any(|a| a == "--help") {
        print_help();
        process::exit(0);
    }

    // Show version and exit if requested.
    if args.iter().any(|a| a == "--version") {
        println!("RevoltBot 1.0.0");
        process::exit(0);
    }

    // Prepare for argument handling.
    let mut env_file: Option<String> = None;
    let mut username: Option<String> = None;
    let mut password: Option<String> = None;
    let mut token: Option<String> = None;
    let mut proxy: Option<String> = None;
    let mut verbose = false;
    let mut config_file: Option<String> = None;
    let mut start_off = false;
    let mut admin_log = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--proxy" | "-x" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("[ERROR] --proxy requires a proxy URL.");
                    process::exit(1);
                }
                proxy = Some(args[i].clone());
            }
            "--env-file" | "-e" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("[ERROR] --env-file requires a file name.");
                    process::exit(1);
                }
                env_file = Some(args[i].clone());
            }
            "--username" | "-u" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("[ERROR] --username requires a username.");
                    process::exit(1);
                }
                username = Some(args[i].clone());
            }
            "--password" | "-p" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("[ERROR] --password requires a password.");
                    process::exit(1);
                }
                password = Some(args[i].clone());
            }
            "--token" | "-t" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("[ERROR] --token requires a token string.");
                    process::exit(1);
                }
                token = Some(args[i].clone());
            }
            "--verbose" | "-v" => {
                verbose = true;
            }
            "--start-off" => {
                start_off = true;
            }
            "--admin-log" => {
                admin_log = true;
            }
            val => {
                // If we already have a config_file assigned, warn about multiple.
                if config_file.is_none() {
                    config_file = Some(val.to_string());
                } else {
                    eprintln!(
                        "[WARNING] Multiple config files specified. Using '{}' and ignoring '{}'.",
                        config_file.as_ref().unwrap(),
                        val
                    );
                }
            }
        }
        i += 1;
    }

    // If we have an env file, load it.
    if let Some(env_path) = env_file {
        if verbose {
            println!("[VERBOSE] Loading environment from file: {}", env_path);
        }
        if let Err(e) = dotenvy::from_filename(&env_path) {
            eprintln!("[ERROR] Failed to load .env file '{}': {:?}", env_path, e);
            process::exit(1);
        }
    }

    // Check for credentials in environment if not provided on CLI.
    if username.is_none() && password.is_none() && token.is_none() {
        let env_user = env::var("REVOLT_USERNAME").ok();
        let env_pass = env::var("REVOLT_PASSWORD").ok();
        let env_token = env::var("REVOLT_TOKEN").ok();

        if let (Some(u), Some(p)) = (env_user, env_pass) {
            username = Some(u);
            password = Some(p);
        } else if let Some(tk) = env_token {
            token = Some(tk);
        }
    }

    // Validate that we either have (username+password) or (token).
    match (&username, &password, &token) {
        (None, None, None) => {
            eprintln!("[ERROR] No credentials found. Provide --username/--password or --token or set them via .env");
            process::exit(1);
        }
        (Some(_), None, _) => {
            eprintln!("[ERROR] --username requires --password.");
            process::exit(1);
        }
        (None, Some(_), _) => {
            eprintln!("[ERROR] --password requires --username.");
            process::exit(1);
        }
        (Some(_), Some(_), Some(_)) => {
            eprintln!("[ERROR] Conflicting credentials: can't use both (username/password) and token at the same time.");
            process::exit(1);
        }
        _ => {}
    }

    // Ensure that a config file was provided.
    let config_file = match config_file {
        Some(cf) => cf,
        None => {
            eprintln!(
                "[ERROR] No config file provided. Provide a JSON config file as an argument."
            );
            process::exit(1);
        }
    };

    // Read the JSON config file.
    let config_data = match fs::read_to_string(&config_file) {
        Ok(data) => data,
        Err(e) => {
            eprintln!(
                "[ERROR] Failed to read config file '{}': {:?}",
                config_file, e
            );
            process::exit(1);
        }
    };

    // Parse the JSON config.
    let parsed_config: BotConfig = match serde_json::from_str(&config_data) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("[ERROR] Invalid JSON config in '{}': {:?}", config_file, e);
            process::exit(1);
        }
    };

    // Extra validation.
    if parsed_config.admin.is_empty() {
        eprintln!("[ERROR] Config validation: 'admin' array cannot be empty.");
        process::exit(1);
    }
    for admin_section in &parsed_config.admin {
        if admin_section.channel.is_empty()
            || admin_section.start.is_empty()
            || admin_section.stop.is_empty()
        {
            eprintln!("[ERROR] Config validation: 'admin' channel and start/stop commands cannot be empty.");
            process::exit(1);
        }
    }
    if parsed_config.claim.is_empty() {
        eprintln!("[ERROR] Config validation: 'claim' array cannot be empty.");
        process::exit(1);
    }
    for claim_section in &parsed_config.claim {
        if claim_section.bot_id.is_empty() || claim_section.startswith.is_empty() {
            eprintln!("[ERROR] Config validation: 'bot_id' or 'startswith' in claim config cannot be empty.");
            process::exit(1);
        }
        match claim_section.reply.r#type.as_str() {
            "fixed" => {
                if claim_section.reply.content.is_none() {
                    eprintln!("[ERROR] Config validation: 'fixed' reply type requires 'content'.");
                    process::exit(1);
                }
            }
            "random_letter" => {}  // OK.
            "channel_number" => {} // OK, no extra `content` needed.
            other => {
                eprintln!("[ERROR] Config validation: unknown reply type '{}'.", other);
                process::exit(1);
            }
        }
    }

    if verbose {
        println!(
            "[VERBOSE] Successfully validated config file: {}",
            config_file
        );
    }

    // -------------------------------
    // Set up shared sleep duration.
    // -------------------------------
    let (sleep_tx, sleep_rx) = watch::channel::<Option<Duration>>(None);
    let sleep_tx = Arc::new(Mutex::new(sleep_tx));

    // Initialize the running state. If --start-off is provided, default to OFF.
    let is_running = Arc::new(Mutex::new(!start_off));

    // Create the client. Adjust base_url, ws_url and proxy as needed.
    let base_url = "https://revolt-api.onech.at".to_string();
    let ws_url = Some("wss://revolt-ws.onech.at/".to_string());
    let client = RevoltClient::new(base_url, ws_url, proxy.clone());

    // Attempt to authenticate.
    if let Some(tk) = token {
        if verbose {
            println!("[VERBOSE] Authenticating via token...");
        }
        client.set_token(Some(tk)).await;
    } else {
        if verbose {
            println!("[VERBOSE] Authenticating via username/password...");
        }
        let un = username.unwrap();
        let pw = password.unwrap();
        if let Err(e) = client.login(&un, &pw, None).await {
            eprintln!("[ERROR] Login failed with username/password: {:?}", e);
            process::exit(1);
        }
    }

    // Wrap the parsed config in an Arc for sharing.
    let config_arc = Arc::new(parsed_config);

    // Prepare our concurrency-safe maps and sets
    let channels_map = Arc::new(Mutex::new(HashMap::new()));
    let server_map = Arc::new(Mutex::new(HashMap::new()));
    let server_names = Arc::new(Mutex::new(HashMap::new()));
    let bot_user_id = Arc::new(Mutex::new(None::<String>));
    let active_claims = Arc::new(Mutex::new(HashMap::<String, ClaimTimingData>::new())); // Initialize active claims map

    // Set up our event handler.
    let handler = MyHandler {
        config: config_arc.clone(),
        is_running: is_running.clone(),
        verbose,
        admin_log,
        sleep_tx: sleep_tx.clone(),
        channels_map: channels_map.clone(),
        server_map: server_map.clone(),
        server_names: server_names.clone(),
        bot_user_id: bot_user_id.clone(),
        active_claims: active_claims.clone(), // Pass the new map
    };

    // Register the event handler with the client.
    if let Err(e) = client.event_handler(handler.clone()).await {
        eprintln!("[ERROR] Failed to add event handler: {:?}", e);
        process::exit(1);
    }

    // If admin logging is enabled, send a dashboard message to all admin channels.
    if admin_log {
        let current_running = *is_running.lock().await;
        let status_icon = if current_running {
            ":green_circle:"
        } else {
            ":red_circle:"
        };
        let status_text = if current_running { "ON" } else { "OFF" };
        for admin in &config_arc.admin {
            let dashboard = format!(
                ":tada: Bot is ready and operational :tada:\n{} Current status: {}\n\
                To start the bot, send `{}` (optionally with a duration in minutes, \
                e.g. `{} 5`) to start the bot indefinitely or for a specified time.\n\
                To stop the bot, send `{}`.",
                status_icon, status_text, admin.start, admin.start, admin.stop
            );
            if let Err(e) = client
                .send_message(&admin.channel, &dashboard, None, None, None)
                .await
            {
                eprintln!(
                    "[ERROR] Failed to send admin dashboard message to channel {}: {:?}",
                    admin.channel, e
                );
            }
        }
    }

    // Spawn a background task that monitors the sleep duration.
    // When a duration is set (Some(duration)), this task sleeps for that duration.
    // If the sleep completes (i.e. no update occurred) the bot is automatically stopped
    // and an admin log message is sent.
    let is_running_clone = is_running.clone();
    let sleep_tx_clone = sleep_tx.clone();
    let mut sleep_rx = sleep_rx;
    let admin_log_flag = admin_log;
    let config_for_sleep = config_arc.clone();
    let client_for_sleep = client.clone();

    tokio::spawn(async move {
        loop {
            let current_value = *sleep_rx.borrow();
            match current_value {
                Some(duration) => {
                    if verbose {
                        println!("[VERBOSE] Bot started for {:?} second(s)", duration);
                    }
                    // Store the original duration, then do a tokio::select!
                    let sleep_duration = duration;
                    tokio::select! {
                        _ = sleep(sleep_duration) => {
                            // The sleep duration has elapsed.
                            {
                                let mut running = is_running_clone.lock().await;
                                *running = false;
                            }
                            if admin_log_flag {
                                let minutes = sleep_duration.as_secs() / 60;
                                let log_msg = format!(
                                    ":alarm_clock: Bot automatically stopped after {} minute(s).",
                                    minutes
                                );
                                // Send the timeout message to all admin channels.
                                for admin in &config_for_sleep.admin {
                                    if let Err(e) = client_for_sleep.send_message(&admin.channel, &log_msg, None, None, None).await {
                                        eprintln!("[ERROR] Failed to send timeout admin log to channel {}: {:?}", admin.channel, e);
                                    }
                                }
                            }
                            // Clear the sleep timer
                            {
                                let tx = sleep_tx_clone.lock().await;
                                if tx.send(None).is_err() {
                                    eprintln!("[ERROR] Failed to clear sleep duration after timeout.");
                                }
                            }
                        }
                        res = sleep_rx.changed() => {
                            if res.is_err() && verbose {
                                eprintln!("[VERBOSE] Sleep watch channel closed. Exiting sleep task.");
                                break;
                            }
                            if verbose {
                                println!("[VERBOSE] Sleep duration updated; aborting current sleep.");
                            }
                            // The sleep duration was updated; re-loop to handle new value.
                        }
                    }
                }
                None => {
                    // Wait for next change if any
                    if sleep_rx.changed().await.is_err() {
                        if verbose {
                            eprintln!("[VERBOSE] Sleep watch channel closed. Exiting sleep task.");
                        }
                        break;
                    }
                }
            }
        }
    });

    // Start the Revolt event loop.
    if verbose {
        println!("[VERBOSE] Starting Revolt event loop...");
    }
    if let Err(e) = client.start().await {
        eprintln!("[ERROR] Failed to start Revolt client event loop: {:?}", e);
        process::exit(1);
    }

    println!("Bot is running. Press Ctrl+C to stop.");
    match tokio::signal::ctrl_c().await {
        Ok(()) => {
            println!("\nShutting down...");
            if let Err(e) = client.close_ws(Some("Shutting down")).await {
                eprintln!("Error during shutdown: {:?}", e);
            }
        }
        Err(e) => eprintln!("Error waiting for Ctrl+C: {:?}", e),
    }
}

/// Print help text and usage examples
fn print_help() {
    println!(
        r#"Usage: revolve_bot [OPTION]... [CONFIG_FILE]
Run a Revolt bot with the specified JSON configuration file.

A valid CONFIG_FILE (in JSON format) is required.

Options:
  -e, --env-file [FILE]        Load environment variables from a .env file.
  -u, --username [USERNAME]    Specify Revolt username (requires --password).
  -p, --password [PASSWORD]    Specify Revolt password (requires --username).
  -t, --token [TOKEN]          Specify Revolt token (cannot be used with -u/-p).
  -x, --proxy [PROXY]          Specify HTTP/WS proxy URL (must start with http:// or https://).
  -v, --verbose                Increase verbosity of output.
      --start-off              Start the bot as not running (OFF). 
                               It will start only when a `.start` command is issued.
      --admin-log              Enable admin dashboard logging 
                               (sends operational and status messages to admin channels).
      --help                   Display this help and exit.
      --version                Output version information and exit.

Examples:
  revolve_bot -e .env config.json
  revolve_bot -u MyUser -p MyPass config.json
  revolve_bot -t MyToken config.json
  revolve_bot -x http://proxy:8080 config.json
"#
    );
}
