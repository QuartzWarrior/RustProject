use serde::{Deserialize, Serialize};

use crate::{
    client::RevoltClient,
    api::messages::MessagesApi,
    error::RevoltError
};

/// For sending a message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataMessageSend {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nonce: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replies: Option<Vec<ReplyIntent>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embeds: Option<Vec<SendableEmbed>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub masquerade: Option<Masquerade>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interactions: Option<Interactions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flags: Option<u32>,
}

/// A full Message object as returned from the API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    #[serde(rename = "_id")]
    pub id: String,
    pub nonce: Option<String>,
    pub channel: String,
    pub author: String,
    #[serde(skip)]
    pub client: Option<RevoltClient>, // We inject a reference to the client after deserialization

    // Additional fields from the schema:
    pub user: Option<crate::types::user::User>,
    pub member: Option<crate::types::user::Member>,
    pub webhook: Option<MessageWebhook>,
    pub content: Option<String>,
    pub system: Option<SystemMessage>,
    pub attachments: Option<Vec<File>>,
    pub edited: Option<String>,  // ISO8601
    pub embeds: Option<Vec<Embed>>,
    pub mentions: Option<Vec<String>>,
    pub replies: Option<Vec<String>>,
    pub reactions: Option<std::collections::HashMap<String, Vec<String>>>,
    pub interactions: Option<Interactions>,
    pub masquerade: Option<Masquerade>,
    pub flags: Option<u32>,
}

impl Message {
    /// Send message
    pub async fn send_message(&self, content: &str, reply: Option<bool>, nonce: Option<String>) -> Result<Message, RevoltError> { // Add nonce parameter
        if let Some(client) = &self.client {
            let replies = if reply.unwrap_or(false) {
                Some(vec![ReplyIntent {
                    id: self.id.clone(),
                    mention: false,
                }])
            } else {
                None
            };

            // Pass nonce along
            client.send_message(
                &self.channel,
                content,
                replies,
                None,
                nonce,
            )
            .await
        } else {
            Err(RevoltError::Other("Client not attached".to_string()))
        }
    }

    /// Reply to this message.
    pub async fn reply(&self, content: &str) -> Result<Message, RevoltError> {
        // Pass None for nonce to generate automatically
        self.send_message(content, Some(true), None).await
    }
}

/* Below are the sub-types from the OpenAPI spec */

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplyIntent {
    pub id: String,
    pub mention: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendableEmbed {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub colour: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Masquerade {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub colour: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Interactions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reactions: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restrict_reactions: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageWebhook {
    pub name: String,
    pub avatar: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
pub enum SystemMessage {
    #[serde(rename = "text")]
    Text { content: String },

    #[serde(rename = "user_added")]
    UserAdded { id: String, by: String },

    #[serde(rename = "user_remove")]
    UserRemove { id: String, by: String },

    #[serde(rename = "user_joined")]
    UserJoined { id: String },

    #[serde(rename = "user_left")]
    UserLeft { id: String },

    #[serde(rename = "user_kicked")]
    UserKicked { id: String },

    #[serde(rename = "user_banned")]
    UserBanned { id: String },

    #[serde(rename = "channel_renamed")]
    ChannelRenamed { name: String, by: String },

    #[serde(rename = "channel_description_changed")]
    ChannelDescriptionChanged { by: String },

    #[serde(rename = "channel_icon_changed")]
    ChannelIconChanged { by: String },

    #[serde(rename = "channel_ownership_changed")]
    ChannelOwnershipChanged { from: String, to: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct File {
    #[serde(rename = "_id")]
    pub id: String,
    pub tag: String,
    pub filename: String,
    pub metadata: Metadata,
    pub content_type: String,
    pub size: i32,
    pub deleted: Option<bool>,
    pub reported: Option<bool>,
    pub message_id: Option<String>,
    pub user_id: Option<String>,
    pub server_id: Option<String>,
    pub object_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
#[serde(rename_all = "PascalCase")]
pub enum Metadata {
    File,
    Text,
    Image { width: u32, height: u32 },
    Video { width: u32, height: u32 },
    Audio,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
#[serde(rename_all = "PascalCase")]
pub enum Embed {
    Website {
        url: Option<String>,
        original_url: Option<String>,
        special: Option<Special>,
        title: Option<String>,
        description: Option<String>,
        image: Option<Image>,
        video: Option<Video>,
        site_name: Option<String>,
        icon_url: Option<String>,
        colour: Option<String>,
    },
    Image {
        url: String,
        width: i32,
        height: i32,
        size: ImageSize,
    },
    Video {
        url: String,
        width: i32,
        height: i32,
    },
    Text {
        icon_url: Option<String>,
        url: Option<String>,
        title: Option<String>,
        description: Option<String>,
        media: Option<File>,
        colour: Option<String>,
    },
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum ImageSize {
    Large,
    Preview,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Image {
    pub url: String,
    pub width: i32,
    pub height: i32,
    pub size: ImageSize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Video {
    pub url: String,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
#[serde(rename_all = "PascalCase")]
pub enum Special {
    None,
    GIF,
    YouTube {
        id: String,
        timestamp: Option<String>,
    },
    Lightspeed {
        content_type: LightspeedType,
        id: String,
    },
    Twitch {
        content_type: TwitchType,
        id: String,
    },
    Spotify {
        content_type: String,
        id: String,
    },
    Soundcloud,
    Bandcamp {
        content_type: BandcampType,
        id: String,
    },
    Streamable {
        id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum LightspeedType {
    Channel,
    Clip,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum TwitchType {
    Channel,
    Clip,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum BandcampType {
    Album,
    Track,
}
