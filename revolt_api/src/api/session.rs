use crate::{
    client::{parse_json_if_ok, RevoltClient},
    error::RevoltError,
    types::session::{SessionInfo, DataEditSession},
    util::build_url,
};

#[async_trait::async_trait]
pub trait SessionApi {
    /// Fetch all sessions associated with this account.
    async fn fetch_all_sessions(&self) -> Result<Vec<SessionInfo>, RevoltError>;

    /// Delete all active sessions, optionally including the current one.
    async fn delete_all_sessions(&self, revoke_self: bool) -> Result<(), RevoltError>;

    /// Delete a specific active session by ID.
    async fn delete_session(&self, id: &str) -> Result<(), RevoltError>;

    /// Edit a specific session by ID (e.g., change friendly name).
    async fn edit_session(&self, id: &str, new_name: &str) -> Result<SessionInfo, RevoltError>;
}

#[async_trait::async_trait]
impl SessionApi for RevoltClient {
    async fn fetch_all_sessions(&self) -> Result<Vec<SessionInfo>, RevoltError> {
        let url = build_url(&self.base_url, &["auth", "session", "all"]);
        let resp = self.authed_get(&url, None).await?;
        parse_json_if_ok(resp).await
    }

    async fn delete_all_sessions(&self, revoke_self: bool) -> Result<(), RevoltError> {
        let url = build_url(&self.base_url, &["auth", "session", "all"]);
        let query = [("revoke_self", revoke_self.to_string())];
        let resp = self.authed_delete_with_query(&url, &query, None).await?;
        if resp.status().is_success() {
            // Clear our local token if revoke_self was true
            if revoke_self {
                self.set_token(None).await;
            }
            Ok(())
        } else {
            Err(RevoltError::HttpStatus {
                code: resp.status().as_u16(),
                body: resp.text().await.unwrap_or_default(),
            })
        }
    }

    async fn delete_session(&self, id: &str) -> Result<(), RevoltError> {
        let url = build_url(&self.base_url, &["auth", "session", id]);
        let resp = self.authed_delete(&url, None).await?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(RevoltError::HttpStatus {
                code: resp.status().as_u16(),
                body: resp.text().await.unwrap_or_default(),
            })
        }
    }

    async fn edit_session(&self, id: &str, new_name: &str) -> Result<SessionInfo, RevoltError> {
        let url = build_url(&self.base_url, &["auth", "session", id]);
        let body = DataEditSession {
            friendly_name: new_name.to_string(),
        };
        let resp = self.authed_patch(&url, &body, None).await?;
        parse_json_if_ok(resp).await
    }
}
