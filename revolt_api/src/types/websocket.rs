use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Represents every possible event the client can send to the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "PascalCase")]
pub enum ClientToServerEvent {
    Authenticate {
        token: String,
    },
    BeginTyping {
        channel: String,
    },
    EndTyping {
        channel: String,
    },
    Ping {
        data: i64,
    },
    Subscribe {
        server_id: String,
    },
    // ... Add other outgoing events if needed ...
}

/// Represents every possible server → client event, based on the Revolt (Bonfire) protocol.
///
/// Where appropriate, we flatten objects or keep them as `serde_json::Value` to allow flexibility.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "PascalCase")]
pub enum ServerToClientEvent {
    /// Server responded with an error on authentication or other issue.
    Error {
        error: String,
    },
    /// Connection has been authenticated successfully.
    Authenticated,
    /// The current session has been invalidated or reset.
    Logout,
    /// A bulk event containing multiple sub-events.
    Bulk {
        v: Vec<ServerToClientEvent>,
    },
    /// Ping response from the server.
    Pong {
        data: i64,
    },

    /// A "Ready" event containing initial data for the client.
    /// In practice, Revolt can also send `members` even though
    /// it's not always in older docs.
    Ready {
        users: Vec<Value>,
        servers: Vec<Value>,
        channels: Vec<Value>,
        #[serde(default)]
        emojis: Vec<Value>,
        /// This is not always present in older specs, so we default it
        /// to an empty array if missing.
        #[serde(default)]
        members: Vec<Value>,
    },

    // -- MESSAGE EVENTS --
    Message {
        #[serde(rename = "_id")]
        id: String,
        channel: String,
        author: String,
        content: Option<String>,
        #[serde(flatten)]
        extra: Value,
    },
    MessageUpdate {
        id: String,
        channel: String,
        #[serde(rename = "data")]
        data: Value,
    },
    MessageAppend {
        id: String,
        channel: String,
        append: Value,
    },
    MessageDelete {
        id: String,
        channel: String,
    },
    MessageReact {
        id: String,
        channel_id: String,
        user_id: String,
        emoji_id: String,
    },
    MessageUnreact {
        id: String,
        channel_id: String,
        user_id: String,
        emoji_id: String,
    },
    MessageRemoveReaction {
        id: String,
        channel_id: String,
        emoji_id: String,
    },

    // -- CHANNEL EVENTS --
    ChannelCreate {
        /// The entire object is flattened into `data`.
        #[serde(flatten)]
        data: Value,
    },
    ChannelUpdate {
        id: String,
        data: Value,
        clear: Option<Vec<String>>,
    },
    ChannelDelete {
        id: String,
    },
    ChannelGroupJoin {
        id: String,
        user: String,
    },
    ChannelGroupLeave {
        id: String,
        user: String,
    },
    ChannelStartTyping {
        id: String,
        user: String,
    },
    ChannelStopTyping {
        id: String,
        user: String,
    },
    ChannelAck {
        id: String,
        user: String,
        message_id: String,
    },

    // -- SERVER EVENTS --
    ServerCreate {
        #[serde(flatten)]
        data: Value,
    },
    ServerUpdate {
        id: String,
        data: Value,
        clear: Option<Vec<String>>,
    },
    ServerDelete {
        id: String,
    },
    ServerMemberUpdate {
        id: ServerMemberIdentifier,
        data: Value,
        clear: Option<Vec<String>>,
    },
    ServerMemberJoin {
        id: String,
        user: String,
    },
    ServerMemberLeave {
        id: String,
        user: String,
    },
    ServerRoleUpdate {
        id: String,
        role_id: String,
        data: Value,
        clear: Option<Vec<String>>,
    },
    ServerRoleDelete {
        id: String,
        role_id: String,
    },

    // -- USER EVENTS --
    UserUpdate {
        id: String,
        data: Value,
        clear: Option<Vec<String>>,
    },
    UserRelationship {
        id: String,
        user: Value,
        status: String,
    },
    UserPlatformWipe {
        user_id: String,
        flags: i64,
    },

    // -- EMOJI EVENTS --
    EmojiCreate {
        #[serde(flatten)]
        data: Value,
    },
    EmojiDelete {
        id: String,
    },

    // -- AUTHIFIER EVENTS --
    Auth {
        event_type: String,
        #[serde(flatten)]
        data: Value,
    },
}

/// A helper struct used for the `ServerMemberUpdate` event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerMemberIdentifier {
    pub server: String,
    pub user: String,
}
