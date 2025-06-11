use crate::{
    client::{parse_json_if_ok, RevoltClient},
    error::RevoltError,
    types::auth::{DataLogin, ResponseLogin},
    util::build_url,
};

#[async_trait::async_trait]
pub trait AuthApi {
    /// Login to an account.
    ///
    /// Returns either `ResponseLogin::Success(...)` or possibly an MFA step, etc.
    async fn login(
        &self,
        email: &str,
        password: &str,
        friendly_name: Option<String>,
    ) -> Result<ResponseLogin, RevoltError>;

    /// Log out of the current session.
    async fn logout(&self) -> Result<(), RevoltError>;
}

#[async_trait::async_trait]
impl AuthApi for RevoltClient {
    async fn login(
        &self,
        email: &str,
        password: &str,
        friendly_name: Option<String>,
    ) -> Result<ResponseLogin, RevoltError> {
        let url = build_url(&self.base_url, &["auth", "session", "login"]);
        let body = DataLogin::Plain {
            email: email.to_string(),
            password: password.to_string(),
            friendly_name,
        };
        let resp = self.http.post(url)
            .json(&body)
            .send()
            .await
            .map_err(RevoltError::ReqwestError)?;

        let login_resp: ResponseLogin = parse_json_if_ok(resp).await?;
        
        // If it's a success variant with a token, set the token in the client
        if let ResponseLogin::Success {
            token, ..
        } = &login_resp
        {
            self.set_token(Some(token.clone())).await;
        }

        Ok(login_resp)
    }

    async fn logout(&self) -> Result<(), RevoltError> {
        let url = build_url(&self.base_url, &["auth", "session", "logout"]);
        let resp = self.authed_post_empty(&url, None).await?;
        // Ensure the response is 204 or handle an error:
        if !resp.status().is_success() {
            return Err(RevoltError::HttpStatus {
                code: resp.status().as_u16(),
                body: resp.text().await.unwrap_or_default(),
            });
        }
        // Clear token
        self.set_token(None).await;
        Ok(())
    }
}
