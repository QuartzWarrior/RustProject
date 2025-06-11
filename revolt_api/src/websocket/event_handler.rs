use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;

use crate::{
    client::RevoltClient,
    types::{
        message::Message,
        websocket::ServerToClientEvent,
    },
};

/// An EventHandler-like trait. This is analogous to Serenity's EventHandler.
#[async_trait]
pub trait EventHandler: Send + Sync + 'static {
    /// Called for *every* event that the server sends.
    /// Useful if you want a single place to inspect all events.
    async fn on_event(&self, _client: &RevoltClient, _event: &ServerToClientEvent) {}

    /// Called specifically when an "Error" event is received
    /// (e.g. a failed authentication).
    async fn on_error_event(&self, _client: &RevoltClient, _error_id: &str) {}

    /// Called specifically when the "Authenticated" event is received
    /// (meaning we successfully authenticated the connection).
    async fn on_authenticated(&self, _client: &RevoltClient) {}

    /// Called when a "Ready" event is received (**includes full payload**).
    async fn on_ready(&self, _client: &RevoltClient, _ready: &ReadyEvent) {}

    // ------------------------------------------------------------------
    // Below are specialized methods for message-related events.
    // If you do not need them, simply omit them (the defaults do nothing).
    // ------------------------------------------------------------------

    /// Called when the server notifies us of a new message.
    async fn on_message(&self, _client: &RevoltClient, _message: &Message) {}

    /// Called when a message is updated/edited.
    async fn on_message_update(&self, _client: &RevoltClient, _update: &MessageUpdateEvent) {}

    /// Called when content is appended to a message (e.g. partial edit).
    async fn on_message_append(&self, _client: &RevoltClient, _append: &MessageAppendEvent) {}

    /// Called when a message is deleted.
    async fn on_message_delete(&self, _client: &RevoltClient, _delete: &MessageDeleteEvent) {}

    /// Called when someone adds a reaction to a message.
    async fn on_message_react(&self, _client: &RevoltClient, _react: &MessageReactEvent) {}

    /// Called when someone removes a particular reaction user from a message.
    async fn on_message_unreact(&self, _client: &RevoltClient, _unreact: &MessageUnreactEvent) {}

    /// Called when *all* instances of a particular emoji reaction are removed.
    async fn on_message_remove_reaction(&self, _client: &RevoltClient, _remove: &MessageRemoveReactionEvent) {}

    // ------------------------------------------------------------------
    // New channel-related events
    // ------------------------------------------------------------------

    /// Called when a new channel is created.
    async fn on_channel_create(&self, _client: &RevoltClient, _event: &ChannelCreateEvent) {}

    /// Called when a channel is updated (renamed, topic changed, etc).
    async fn on_channel_update(&self, _client: &RevoltClient, _update: &ChannelUpdateEvent) {}

    /// Called when a channel is deleted.
    async fn on_channel_delete(&self, _client: &RevoltClient, _delete: &ChannelDeleteEvent) {}
}

/// Data for a "Ready" event, containing the full payload.
///
/// Revolt may include `members` in the payload even though
/// it's not always documented in older specs.
#[derive(Debug, Clone)]
pub struct ReadyEvent {
    pub users: Vec<Value>,
    pub servers: Vec<Value>,
    pub channels: Vec<Value>,
    pub emojis: Vec<Value>,
    pub members: Vec<Value>,
}

// ------------------------------------------------------
// Message-related event data
// ------------------------------------------------------

/// Data for a "MessageUpdate" event.
#[derive(Debug, Clone)]
pub struct MessageUpdateEvent {
    pub id: String,
    pub channel: String,
    /// Contains the fields that were updated, in JSON form.
    pub data: Value,
}

/// Data for a "MessageAppend" event.
#[derive(Debug, Clone)]
pub struct MessageAppendEvent {
    pub id: String,
    pub channel: String,
    /// The appended content, in JSON form.
    pub append: Value,
}

/// Data for a "MessageDelete" event.
#[derive(Debug, Clone)]
pub struct MessageDeleteEvent {
    pub id: String,
    pub channel: String,
}

/// Data for a "MessageReact" event.
#[derive(Debug, Clone)]
pub struct MessageReactEvent {
    pub id: String,
    pub channel_id: String,
    pub user_id: String,
    pub emoji_id: String,
}

/// Data for a "MessageUnreact" event.
#[derive(Debug, Clone)]
pub struct MessageUnreactEvent {
    pub id: String,
    pub channel_id: String,
    pub user_id: String,
    pub emoji_id: String,
}

/// Data for a "MessageRemoveReaction" event.
#[derive(Debug, Clone)]
pub struct MessageRemoveReactionEvent {
    pub id: String,
    pub channel_id: String,
    pub emoji_id: String,
}

// ------------------------------------------------------
// Channel-related event data
// ------------------------------------------------------

/// Data for a "ChannelCreate" event.
#[derive(Debug, Clone, Deserialize)]
pub struct ChannelCreateEvent {
    #[serde(rename = "_id")]
    pub id: String,

    #[serde(default)]
    pub channel_type: String,

    #[serde(default)]
    pub server: Option<String>,

    #[serde(default)]
    pub name: Option<String>,

    #[serde(default)]
    pub description: Option<String>,

    #[serde(default)]
    pub nsfw: Option<bool>,

    /// Catch-all for any other JSON fields you might not have typed out.
    #[serde(flatten)]
    pub other: Value,
}

/// Data for a "ChannelUpdate" event.
///
/// Revolt can send partial updates in the `data` field, plus optional `clear` fields.
#[derive(Debug, Clone)]
pub struct ChannelUpdateEvent {
    pub id: String,
    /// The partial channel object containing updated fields.
    pub data: Value,
    /// List of fields to clear (e.g. "Description", "Icon").
    pub clear: Vec<String>,
}

/// Data for a "ChannelDelete" event.
#[derive(Debug, Clone)]
pub struct ChannelDeleteEvent {
    pub id: String,
}
