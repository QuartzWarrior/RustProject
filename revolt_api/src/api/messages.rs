use crate::{
    client::{parse_json_if_ok, RevoltClient},
    error::RevoltError,
    types::{
        bulk_message_response::BulkMessageResponse,
        message::{DataMessageSend, Message, ReplyIntent},
    },
    util::build_url,
};
use async_trait::async_trait;
use serde::Serialize;
use ulid::Ulid; // Import Ulid

#[derive(Debug, Default, Serialize)]
pub struct FetchMessagesOptions {
    pub limit: Option<i64>,
    pub before: Option<String>,
    pub after: Option<String>,
    pub sort: Option<String>, // e.g. "Relevance","Latest","Oldest"
    pub nearby: Option<String>,
    pub include_users: Option<bool>,
}

/// Trait that holds the methods for message endpoints.
#[async_trait]
pub trait MessagesApi {
    /// Fetch multiple messages from the given channel.
    async fn fetch_messages(
        &self,
        channel_id: &str,
        opts: Option<FetchMessagesOptions>,
    ) -> Result<BulkMessageResponse, RevoltError>;

    /// Send a message to the given channel (basic usage).
    async fn send_message(
        &self,
        channel_id: &str,
        content: &str,
        replies: Option<Vec<ReplyIntent>>,
        idempotency_key: Option<String>,
        nonce: Option<String>, // Add nonce parameter
    ) -> Result<Message, RevoltError>;

    /// Send an OPTIONS request to the send message endpoint to potentially pre-warm the connection.
    async fn prepare_send_message(&self, channel_id: &str) -> Result<(), RevoltError>;
}

#[async_trait]
impl MessagesApi for RevoltClient {
    async fn fetch_messages(
        &self,
        channel_id: &str,
        opts: Option<FetchMessagesOptions>,
    ) -> Result<BulkMessageResponse, RevoltError> {
        let mut url = build_url(&self.base_url, &["channels", channel_id, "messages"]);

        let query = if let Some(o) = opts {
            serde_urlencoded::to_string(o).unwrap_or_default()
        } else {
            String::new()
        };
        if !query.is_empty() {
            url.push('?');
            url.push_str(&query);
        }

        let resp = self.authed_get(&url, None).await?;
        let parsed: BulkMessageResponse = parse_json_if_ok(resp).await?;

        // Attach the client reference to each message (OO style), if needed
        let mut processed = parsed;
        processed.inject_client(self.clone());
        Ok(processed)
    }

    async fn send_message(
        &self,
        channel_id: &str,
        content: &str,
        replies: Option<Vec<ReplyIntent>>,
        idempotency_key: Option<String>,
        nonce: Option<String>, // Add nonce parameter
    ) -> Result<Message, RevoltError> {
        let url = build_url(&self.base_url, &["channels", channel_id, "messages"]);

        // Generate nonce if not provided
        let final_nonce = nonce.unwrap_or_else(|| Ulid::new().to_string());

        let body = DataMessageSend {
            nonce: Some(final_nonce),
            content: Some(content.to_string()),
            attachments: None,
            replies,
            embeds: None,
            masquerade: None,
            interactions: None,
            flags: None,
        };

        // Collect optional headers
        let mut headers = Vec::new();
        if let Some(ref key) = idempotency_key {
            headers.push(("Idempotency-Key", key.as_str()));
        }

        // Send POST with optional headers
        let resp = self.authed_post(&url, &body, Some(&headers)).await?;

        let mut message: Message = parse_json_if_ok(resp).await?;
        message.client = Some(self.clone());
        Ok(message)
    }

    async fn prepare_send_message(&self, channel_id: &str) -> Result<(), RevoltError> {
        let url = build_url(&self.base_url, &["channels", channel_id, "messages"]);

        // Send a non-authed OPTIONS request using the correct http client field
        let resp = self.http.request(reqwest::Method::OPTIONS, &url).send().await;

        match resp {
            Ok(_) => {
                Ok(())    
            }
            Err(e) => {
                Err(RevoltError::ReqwestError(e))
            }
        }
    }
}
