use serde::{Deserialize, Serialize};

use super::{message::Message, user::{User, Member}};

/// Bulk Message Response can be:
/// 1) An array of messages
/// 2) An object: { messages: [...], users: [...], members: [...] }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BulkMessageResponse {
    Messages(Vec<Message>),
    MessagesWithUsers {
        messages: Vec<Message>,
        users: Vec<User>,
        members: Option<Vec<Member>>,
    },
}

impl BulkMessageResponse {
    /// Utility to inject the client reference into each `Message`.
    pub fn inject_client(&mut self, client: crate::RevoltClient) {
        match self {
            Self::Messages(vec) => {
                for m in vec {
                    m.client = Some(client.clone());
                }
            }
            Self::MessagesWithUsers { messages, .. } => {
                for m in messages {
                    m.client = Some(client.clone());
                }
            }
        }
    }
}
