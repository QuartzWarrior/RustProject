//! WebSocket implementation (hand‑rolled handshake to keep dependencies slim).
//! When a proxy is configured in [`RevoltClient::proxy`], the socket is
//! established through an HTTP `CONNECT` tunnel before any TLS upgrade or
//! WebSocket handshake takes place.

use std::{sync::Arc, time::Duration};

use async_recursion::async_recursion;
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use futures::{
    stream::{SplitSink, SplitStream},
    SinkExt, StreamExt,
};
use sha1::{Digest, Sha1};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpStream,
    time::{interval, sleep},
};

use tokio_rustls::{
    rustls::{pki_types::ServerName, ClientConfig, RootCertStore},
    TlsConnector,
};
use tokio_tungstenite::{
    tungstenite::{
        handshake::client::generate_key,
        protocol::{CloseFrame, Message as WsMessage, Role},
    },
    MaybeTlsStream, WebSocketStream,
};
use tungstenite::http::Uri;
use url::Url;

use crate::{
    client::RevoltClient,
    error::RevoltError,
    event_handler::{
        ChannelCreateEvent, ChannelDeleteEvent, ChannelUpdateEvent, EventHandler,
        MessageAppendEvent, MessageDeleteEvent, MessageReactEvent, MessageRemoveReactionEvent,
        MessageUnreactEvent, MessageUpdateEvent, ReadyEvent,
    },
    types::message::Message as RevoltMessage,
    ClientToServerEvent, ServerToClientEvent,
};

/// Connection state for the WebSocket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Closing,
    Reconnecting,
}

const LINUX_USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0";
const WINDOWS_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/135.0.0.0 Safari/537.36";

impl RevoltClient {
    /* ───────────────────── Public API (unchanged) ───────────────────── */

    pub async fn event_handler<E: EventHandler>(&self, handler: E) -> Result<(), RevoltError> {
        *self.event_handler.lock().await = Some(Arc::new(handler));
        Ok(())
    }

    pub async fn start(&self) -> Result<(), RevoltError> {
        if self.ws_tx.lock().await.is_some() {
            return Err(RevoltError::Other(
                "WebSocket is already running on this client!".into(),
            ));
        }

        *self.connection_state.lock().await = ConnectionState::Connecting;

        let ws_url = self.ws_url.as_deref().unwrap_or("wss://ws.revolt.chat/");

        match self.connect_ws(ws_url).await {
            Ok((write, read)) => {
                *self.ws_tx.lock().await = Some(write);
                *self.connection_state.lock().await = ConnectionState::Connected;

                if let Some(token) = self.token.lock().await.clone() {
                    self.send_authenticate(&token).await?;
                }

                /* ─────────── Spawn ping & read‑loop tasks ─────────── */
                tokio::spawn({
                    let client = self.clone();
                    async move {
                        let mut ticker = interval(Duration::from_secs(30));
                        loop {
                            ticker.tick().await;
                            if client.ping(None).await.is_err() {
                                eprintln!("[WS] Ping failed – terminating heartbeat.");
                                break;
                            }
                        }
                    }
                });

                tokio::spawn({
                    let client = self.clone();
                    async move { client.ws_read_loop_with_reconnect(read).await }
                });

                Ok(())
            }
            Err(e) => {
                *self.connection_state.lock().await = ConnectionState::Disconnected;
                Err(e)
            }
        }
    }

    /* ────────────────────────── Socket setup ────────────────────────── */

    /// Establish the underlying TCP/TLS stream, perform the WebSocket handshake
    /// and split it into writer / reader halves.
    async fn connect_ws(
        &self,
        ws_url: &str,
    ) -> Result<
        (
            SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, WsMessage>,
            SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>,
        ),
        RevoltError,
    > {
        /* —— Parse target URI —— */
        let uri: Uri = ws_url
            .parse()
            .map_err(|e| RevoltError::Other(format!("Invalid WebSocket URL: {e}")))?;

        let scheme = uri
            .scheme_str()
            .ok_or_else(|| RevoltError::Other("WebSocket URL missing scheme".into()))?;
        let host = uri
            .host()
            .ok_or_else(|| RevoltError::Other("WebSocket URL missing host".into()))?;
        let port = uri.port_u16().unwrap_or(match scheme {
            "ws" => 80,
            "wss" => 443,
            _ => {
                return Err(RevoltError::Other(format!(
                    "Unsupported WebSocket scheme: {scheme}"
                )))
            }
        });

        let user_agent = if cfg!(target_os = "windows") {
            WINDOWS_USER_AGENT
        } else {
            LINUX_USER_AGENT
        };

        /* —— Establish TCP stream (optionally via proxy) —— */
        let tcp_stream = if let Some(proxy_raw) = &self.proxy {
            // Ensure scheme for URL parser.
            let full = if proxy_raw.starts_with("http://") || proxy_raw.starts_with("https://") {
                proxy_raw.clone()
            } else {
                format!("http://{proxy_raw}")
            };

            let proxy_url = Url::parse(&full)
                .map_err(|e| RevoltError::Other(format!("Invalid proxy URL: {e}")))?;

            let proxy_host = proxy_url
                .host_str()
                .ok_or_else(|| RevoltError::Other("Proxy URL missing host".into()))?;
            let proxy_port = proxy_url
                .port_or_known_default()
                .ok_or_else(|| RevoltError::Other("Proxy URL missing port".into()))?;

            let proxy_addr = format!("{proxy_host}:{proxy_port}");
            let mut stream = TcpStream::connect(&proxy_addr).await.map_err(|e| {
                RevoltError::Other(format!("Failed to connect to proxy {proxy_addr}: {e}"))
            })?;

            /* —— Send HTTP CONNECT —— */
            let connect_target = format!("{host}:{port}");
            let mut connect_req = format!(
                "CONNECT {connect_target} HTTP/1.1\r\nHost: {connect_target}\r\nUser-Agent: {user_agent}\r\n"
            );

            if !proxy_url.username().is_empty() {
                if let Some(pass) = proxy_url.password() {
                    let creds =
                        BASE64_STANDARD.encode(format!("{}:{}", proxy_url.username(), pass));
                    connect_req.push_str(&format!("Proxy-Authorization: Basic {creds}\r\n"));
                }
            }
            connect_req.push_str("Proxy-Connection: Keep-Alive\r\n\r\n");

            stream
                .write_all(connect_req.as_bytes())
                .await
                .map_err(|e| RevoltError::Other(format!("Failed to send CONNECT: {e}")))?;
            stream
                .flush()
                .await
                .map_err(|e| RevoltError::Other(format!("Failed to flush CONNECT: {e}")))?;

            // Parse CONNECT response.
            let mut rdr = BufReader::new(stream);
            let mut status_line = String::new();
            rdr.read_line(&mut status_line)
                .await
                .map_err(|e| RevoltError::Other(format!("Proxy response read error: {e}")))?;

            if !status_line.starts_with("HTTP/1.1 200") && !status_line.starts_with("HTTP/1.0 200")
            {
                return Err(RevoltError::Other(format!(
                    "Proxy tunnel failed: {status_line}"
                )));
            }

            // Consume headers.
            loop {
                let mut line = String::new();
                let bytes = rdr
                    .read_line(&mut line)
                    .await
                    .map_err(|e| RevoltError::Other(format!("Proxy header read error: {e}")))?;
                if bytes == 0 || line == "\r\n" {
                    break;
                }
            }

            rdr.into_inner()
        } else {
            let direct_addr = format!("{host}:{port}");
            TcpStream::connect(&direct_addr).await.map_err(|e| {
                RevoltError::Other(format!("Failed to connect TCP to {direct_addr}: {e}"))
            })?
        };

        /* —— Upgrade to TLS if needed (using rustls) —— */
        let stream: MaybeTlsStream<TcpStream> = match scheme {
            "ws" => MaybeTlsStream::Plain(tcp_stream),
            "wss" => {
                // Configure rustls client
                let mut root_cert_store = RootCertStore::empty();
                root_cert_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

                let config = ClientConfig::builder()
                    .with_root_certificates(root_cert_store)
                    .with_no_client_auth();

                let connector = TlsConnector::from(Arc::new(config));

                // Convert host string to ServerName
                let server_name = ServerName::try_from(host)
                    .map_err(|e| RevoltError::Other(format!("Invalid DNS name '{host}': {e}")))?
                    .to_owned();

                let tls = connector
                    .connect(server_name, tcp_stream)
                    .await
                    .map_err(|e| {
                        RevoltError::Other(format!("TLS (rustls) handshake to {host} failed: {e}"))
                    })?;
                MaybeTlsStream::Rustls(tls) // Use MaybeTlsStream::Rustls
            }
            _ => unreachable!(),
        };

        /* —— Perform WebSocket handshake —— */
        let key = generate_key();
        let path_and_query = uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("/");
        let host_header = uri.authority().unwrap().as_str();

        let mut request = format!("GET {path_and_query} HTTP/1.1\r\n");
        request.push_str(&format!("Host: {host_header}\r\n"));
        request.push_str(&format!("User-Agent: {user_agent}\r\n"));
        request.push_str("Accept: */*\r\n");
        request.push_str("Accept-Language: en-US,en;q=0.5\r\n");
        request.push_str("Accept-Encoding: gzip, deflate, br, zstd\r\n");
        request.push_str("Sec-WebSocket-Version: 13\r\n");
        request.push_str("Origin: https://revolt.onech.at\r\n");
        request.push_str("Sec-WebSocket-Extensions: permessage-deflate\r\n");
        request.push_str(&format!("Sec-WebSocket-Key: {key}\r\n"));
        request.push_str("DNT: 1\r\n");
        request.push_str("Sec-GPC: 1\r\n");
        request.push_str("Connection: Upgrade\r\n");
        request.push_str("Upgrade: websocket\r\n");
        request.push_str("Sec-Fetch-Dest: empty\r\n");
        request.push_str("Sec-Fetch-Mode: websocket\r\n");
        request.push_str("Sec-Fetch-Site: same-site\r\n");
        request.push_str("Pragma: no-cache\r\n");
        request.push_str("Cache-Control: no-cache\r\n\r\n");

        let mut stream = stream;
        stream
            .write_all(request.as_bytes())
            .await
            .map_err(|e| RevoltError::Other(format!("WS handshake send error: {e}")))?;
        stream
            .flush()
            .await
            .map_err(|e| RevoltError::Other(format!("WS handshake flush error: {e}")))?;

        // Read handshake response.
        let mut rdr = BufReader::new(stream);
        let mut response_lines = Vec::new();
        loop {
            let mut line = String::new();
            let read = rdr
                .read_line(&mut line)
                .await
                .map_err(|e| RevoltError::Other(format!("WS handshake read error: {e}")))?;
            if read == 0 {
                return Err(RevoltError::Other(
                    "Connection closed during handshake".into(),
                ));
            }
            if line == "\r\n" {
                break;
            }
            response_lines.push(line.trim_end().to_owned());
        }

        if response_lines.is_empty() {
            return Err(RevoltError::Other("Empty handshake response".into()));
        }
        if !response_lines[0].starts_with("HTTP/1.1 101") {
            return Err(RevoltError::Other(format!(
                "Unexpected handshake status: {}",
                response_lines[0]
            )));
        }

        /* —— Validate essential headers —— */
        let mut upgrade_ok = false;
        let mut connection_ok = false;
        let mut accept_key_ok = false;

        let expected_accept = {
            let mut sha1 = Sha1::new();
            sha1.update(key.as_bytes());
            sha1.update(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
            BASE64_STANDARD.encode(sha1.finalize())
        };

        for line in &response_lines[1..] {
            if let Some((h, v)) = line.split_once(": ") {
                match h.to_ascii_lowercase().as_str() {
                    "upgrade" if v.eq_ignore_ascii_case("websocket") => upgrade_ok = true,
                    "connection" if v.eq_ignore_ascii_case("upgrade") => connection_ok = true,
                    "sec-websocket-accept" if v == expected_accept => accept_key_ok = true,
                    _ => {}
                }
            }
        }

        if !(upgrade_ok && connection_ok && accept_key_ok) {
            return Err(RevoltError::Other(
                "Malformed WebSocket handshake response".into(),
            ));
        }

        /* —— Promote raw stream into tokio‑tungstenite WebSocket —— */
        let ws_stream =
            WebSocketStream::from_raw_socket(rdr.into_inner(), Role::Client, None).await;

        Ok(ws_stream.split())
    }

    /// Main WebSocket read loop with automatic reconnection handling
    async fn ws_read_loop_with_reconnect(
        &self,
        read: SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>,
    ) {
        self.read_loop(read).await;

        let max_retry_count = 10;
        let mut retry_count = 0;
        let mut retry_delay = Duration::from_secs(1);
        let max_delay = Duration::from_secs(60);

        loop {
            let current_state = *self.connection_state.lock().await;
            if current_state == ConnectionState::Closing {
                eprintln!("[WS] Closing connection, not reconnecting");

                let mut state_guard = self.connection_state.lock().await;
                *state_guard = ConnectionState::Disconnected;

                break;
            }

            {
                let mut state_guard = self.connection_state.lock().await;
                *state_guard = ConnectionState::Reconnecting;
            }

            eprintln!(
                "[WS] Connection lost. Attempting to reconnect (attempt {}/{}) in {:?}...",
                retry_count + 1,
                max_retry_count,
                retry_delay
            );

            sleep(retry_delay).await;

            let ws_url = self.ws_url.as_deref().unwrap_or("wss://ws.revolt.chat/");

            match self.connect_ws(ws_url).await {
                Ok((write, read)) => {
                    {
                        let mut guard = self.ws_tx.lock().await;
                        *guard = Some(write);
                    }

                    {
                        let mut state_guard = self.connection_state.lock().await;
                        *state_guard = ConnectionState::Connected;
                    }

                    if let Some(token) = self.token.lock().await.clone() {
                        if let Err(e) = self.send_authenticate(&token).await {
                            eprintln!("[WS] Failed to re-authenticate after reconnection: {e}");

                            let mut guard = self.ws_tx.lock().await;
                            *guard = None;

                            retry_count += 1;
                            retry_delay = std::cmp::min(retry_delay * 2, max_delay);
                            continue;
                        }
                    }

                    eprintln!("[WS] Successfully reconnected!");

                    retry_count = 0;
                    retry_delay = Duration::from_secs(1);

                    self.read_loop(read).await;
                }
                Err(e) => {
                    eprintln!("[WS] Reconnection attempt failed: {e}");

                    retry_count += 1;
                    if retry_count >= max_retry_count {
                        eprintln!("[WS] Maximum reconnection attempts reached. Giving up.");

                        let mut state_guard = self.connection_state.lock().await;
                        *state_guard = ConnectionState::Disconnected;

                        break;
                    }

                    retry_delay = std::cmp::min(retry_delay * 2, max_delay);
                }
            }
        }
    }

    /// The main read loop that receives messages from the server.
    /// We parse them into events, then forward them to the user-provided
    /// handler if available. We also handle WS close / error scenarios.
    async fn read_loop(&self, mut read: SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>) {
        while let Some(msg) = read.next().await {
            let Ok(msg) = msg else {
                eprintln!("[WS] Error reading message. Terminating read loop.");
                break;
            };

            match msg {
                WsMessage::Text(txt) => match serde_json::from_str::<ServerToClientEvent>(&txt) {
                    Ok(event) => {
                        self.handle_event(event).await;
                    }
                    Err(err) => {
                        eprintln!("[WS] Failed to deserialize event: {err}. Raw text: {txt}");
                    }
                },
                WsMessage::Binary(bin) => {
                    match rmp_serde::from_slice::<ServerToClientEvent>(&bin) {
                        Ok(event) => {
                            self.handle_event(event).await;
                        }
                        Err(err) => {
                            eprintln!("[WS] Failed to deserialize MsgPack event: {err:?}");
                        }
                    }
                }
                WsMessage::Close(cf) => {
                    eprintln!("[WS] Close frame received: {:?}", cf);
                    break;
                }
                WsMessage::Ping(_) | WsMessage::Pong(_) | WsMessage::Frame(_) => {}
            }
        }

        let mut guard = self.ws_tx.lock().await;
        *guard = None;
    }

    /// Get the current WebSocket connection state
    pub async fn connection_state(&self) -> ConnectionState {
        *self.connection_state.lock().await
    }

    /// Recursively handle events, including "Bulk" which contains multiple sub-events.
    #[async_recursion]
    async fn handle_event(&self, event: ServerToClientEvent) {
        if let ServerToClientEvent::Bulk { v } = event {
            for sub_event in v {
                self.handle_event(sub_event).await;
            }
            return;
        }

        let maybe_handler = {
            let guard = self.event_handler.lock().await;
            guard.clone()
        };

        let Some(handler) = maybe_handler else {
            return;
        };

        handler.on_event(self, &event).await;

        match &event {
            ServerToClientEvent::Error { error } => {
                handler.on_error_event(self, error).await;
            }
            ServerToClientEvent::Authenticated => {
                handler.on_authenticated(self).await;
            }
            ServerToClientEvent::Ready {
                users,
                servers,
                channels,
                emojis,
                members,
            } => {
                let evt_data = ReadyEvent {
                    users: users.clone(),
                    servers: servers.clone(),
                    channels: channels.clone(),
                    emojis: emojis.clone(),
                    members: members.clone(),
                };
                handler.on_ready(self, &evt_data).await;
            }

            // ------------------ MESSAGE EVENTS -------------------
            ServerToClientEvent::Message {
                id,
                channel,
                author,
                content,
                extra,
            } => {
                let mut payload = serde_json::json!({
                    "_id": id,
                    "channel": channel,
                    "author": author,
                    "content": content,
                });

                if let Some(obj) = payload.as_object_mut() {
                    if let Some(extra_obj) = extra.as_object() {
                        for (k, v) in extra_obj.iter() {
                            obj.insert(k.clone(), v.clone());
                        }
                    }
                }

                let mut message: RevoltMessage = match serde_json::from_value(payload) {
                    Ok(m) => m,
                    Err(e) => {
                        eprintln!("[WS] Could not parse event into `Message`: {:?}", e);
                        return;
                    }
                };

                message.client = Some(self.clone());

                handler.on_message(self, &message).await;
            }
            ServerToClientEvent::MessageUpdate { id, channel, data } => {
                let evt = MessageUpdateEvent {
                    id: id.clone(),
                    channel: channel.clone(),
                    data: data.clone(),
                };
                handler.on_message_update(self, &evt).await;
            }
            ServerToClientEvent::MessageAppend {
                id,
                channel,
                append,
            } => {
                let evt = MessageAppendEvent {
                    id: id.clone(),
                    channel: channel.clone(),
                    append: append.clone(),
                };
                handler.on_message_append(self, &evt).await;
            }
            ServerToClientEvent::MessageDelete { id, channel } => {
                let evt = MessageDeleteEvent {
                    id: id.clone(),
                    channel: channel.clone(),
                };
                handler.on_message_delete(self, &evt).await;
            }
            ServerToClientEvent::MessageReact {
                id,
                channel_id,
                user_id,
                emoji_id,
            } => {
                let evt = MessageReactEvent {
                    id: id.clone(),
                    channel_id: channel_id.clone(),
                    user_id: user_id.clone(),
                    emoji_id: emoji_id.clone(),
                };
                handler.on_message_react(self, &evt).await;
            }
            ServerToClientEvent::MessageUnreact {
                id,
                channel_id,
                user_id,
                emoji_id,
            } => {
                let evt = MessageUnreactEvent {
                    id: id.clone(),
                    channel_id: channel_id.clone(),
                    user_id: user_id.clone(),
                    emoji_id: emoji_id.clone(),
                };
                handler.on_message_unreact(self, &evt).await;
            }
            ServerToClientEvent::MessageRemoveReaction {
                id,
                channel_id,
                emoji_id,
            } => {
                let evt = MessageRemoveReactionEvent {
                    id: id.clone(),
                    channel_id: channel_id.clone(),
                    emoji_id: emoji_id.clone(),
                };
                handler.on_message_remove_reaction(self, &evt).await;
            }

            // ------------------ CHANNEL EVENTS -------------------
            ServerToClientEvent::ChannelCreate { data } => {
                match serde_json::from_value::<ChannelCreateEvent>(data.clone()) {
                    Ok(parsed) => handler.on_channel_create(self, &parsed).await,
                    Err(e) => eprintln!("[WS] Could not parse ChannelCreate: {e}"),
                }
            }
            ServerToClientEvent::ChannelUpdate { id, data, clear } => {
                let evt = ChannelUpdateEvent {
                    id: id.clone(),
                    data: data.clone(),
                    clear: clear.clone().unwrap_or_default(),
                };
                handler.on_channel_update(self, &evt).await;
            }
            ServerToClientEvent::ChannelDelete { id } => {
                let evt = ChannelDeleteEvent { id: id.clone() };
                handler.on_channel_delete(self, &evt).await;
            }

            _ => {}
        }
    }

    /// Send an `Authenticate` event with the given token.
    pub async fn send_authenticate(&self, token: &str) -> Result<(), RevoltError> {
        self.send_ws(ClientToServerEvent::Authenticate {
            token: token.to_string(),
        })
        .await
    }

    /// Send a Ping to the server. If `data` is None, defaults to 0.
    pub async fn ping(&self, data: Option<i64>) -> Result<(), RevoltError> {
        self.send_ws(ClientToServerEvent::Ping {
            data: data.unwrap_or(0),
        })
        .await
    }

    /// Send a `BeginTyping` event.
    pub async fn begin_typing(&self, channel_id: &str) -> Result<(), RevoltError> {
        self.send_ws(ClientToServerEvent::BeginTyping {
            channel: channel_id.to_string(),
        })
        .await
    }

    /// Send an `EndTyping` event.
    pub async fn end_typing(&self, channel_id: &str) -> Result<(), RevoltError> {
        self.send_ws(ClientToServerEvent::EndTyping {
            channel: channel_id.to_string(),
        })
        .await
    }

    /// Send a `Subscribe` event to subscribe to user updates in a server.
    pub async fn subscribe(&self, server_id: &str) -> Result<(), RevoltError> {
        self.send_ws(ClientToServerEvent::Subscribe {
            server_id: server_id.to_string(),
        })
        .await
    }

    async fn send_ws(&self, payload: ClientToServerEvent) -> Result<(), RevoltError> {
        let mut guard = self.ws_tx.lock().await;
        let Some(writer) = guard.as_mut() else {
            return Err(RevoltError::Other("WebSocket not connected!".into()));
        };

        let text = serde_json::to_string(&payload)
            .map_err(|e| RevoltError::Other(format!("Failed to serialize WS payload: {e}")))?;
        writer
            .send(WsMessage::Text(text.into()))
            .await
            .map_err(|e| RevoltError::Other(format!("Failed to send WS message: {e}")))?;
        Ok(())
    }

    pub async fn close_ws(&self, reason: Option<&str>) -> Result<(), RevoltError> {
        {
            let mut state_guard = self.connection_state.lock().await;
            *state_guard = ConnectionState::Closing;
        }

        let mut guard = self.ws_tx.lock().await;
        if let Some(writer) = guard.as_mut() {
            let frame = CloseFrame {
                code: tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::Normal,
                reason: reason.unwrap_or("Closing").into(),
            };
            writer
                .send(WsMessage::Close(Some(frame)))
                .await
                .map_err(|e| RevoltError::Other(format!("Error sending WS close frame: {e}")))?;
        }
        *guard = None;
        Ok(())
    }
}
