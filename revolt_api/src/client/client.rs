//! Revolt HTTP‑and‑WebSocket client.
//!
//! The client now supports an **optional HTTP proxy** used **for *all* HTTP
//! requests and WebSocket traffic**.  
//! Supported proxy formats:
//! * `http://USERNAME:PASSWORD@IP:PORT`  
//! * `http://IP:PORT` *(user / password omitted)*

use std::fmt::{self, Debug, Formatter};
use std::sync::Arc;
use std::time::Duration;

use reqwest::{Client, ClientBuilder, Method, Proxy, Response};
use tokio::{net::TcpStream, sync::Mutex};

use futures::stream::SplitSink;
use tokio_tungstenite::{
    tungstenite::protocol::Message as WsMessage, MaybeTlsStream, WebSocketStream,
};

use crate::error::{handle_api_error, RevoltError};
use crate::event_handler::EventHandler;
use crate::websocket::websocket::ConnectionState;

/// Main client to interact with the Revolt API.
#[derive(Clone)]
pub struct RevoltClient {
    /* ───────────────────────── Public configuration ───────────────────────── */
    pub base_url: String,
    pub ws_url: Option<String>,
    /// Optional HTTP proxy – *must* start with `http://` or `https://`.
    pub proxy: Option<String>,

    /* ───────────────────────── Internal plumbing ──────────────────────────── */
    pub http: Client,
    pub token: Arc<Mutex<Option<String>>>,
    // Update WebSocketStream generic type to use tokio_rustls::client::TlsStream
    pub ws_tx: Arc<Mutex<Option<SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, WsMessage>>>>,
    pub event_handler: Arc<Mutex<Option<Arc<dyn EventHandler>>>>,
    pub connection_state: Arc<Mutex<ConnectionState>>,
}

impl Debug for RevoltClient {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("RevoltClient")
            .field("base_url", &self.base_url)
            .field("ws_url", &self.ws_url)
            .field("proxy", &self.proxy)
            .field("http", &"reqwest::Client")
            .field("token", &self.token)
            .field(
                "event_handler",
                &"Arc<Mutex<Option<Arc<dyn EventHandler>>>>",
            )
            .field("connection_state", &self.connection_state)
            .finish()
    }
}

const LINUX_USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0";
const WINDOWS_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/135.0.0.0 Safari/537.36";

impl RevoltClient {
    /// Construct a new [`RevoltClient`].
    ///
    /// # Parameters
    /// * `base_url` – REST endpoint (e.g. `https://api.revolt.chat`).
    /// * `ws_url`   – WebSocket endpoint (`wss://…`); if `None`, defaults to
    ///   `wss://ws.revolt.chat/`.
    /// * `proxy`    – **Optional** proxy URL, accepted in the formats described
    ///   at the top of this file.
    pub fn new(base_url: String, ws_url: Option<String>, proxy: Option<String>) -> Self {
        let mut builder = ClientBuilder::new()
            .pool_max_idle_per_host(10)
            .pool_idle_timeout(Duration::from_secs(90))
            .http2_prior_knowledge()
            .use_rustls_tls();

        if let Some(ref p) = proxy {
            let full = if p.starts_with("http://") || p.starts_with("https://") {
                p.clone()
            } else {
                format!("http://{p}")
            };

            let req_proxy =
                Proxy::all(&full).unwrap_or_else(|e| panic!("Invalid proxy URL `{full}`: {e}"));
            builder = builder.proxy(req_proxy);
        }

        let http = builder.build().expect("Failed to build reqwest client");

        Self {
            base_url,
            ws_url,
            proxy,
            http,
            token: Arc::new(Mutex::new(None)),
            ws_tx: Arc::new(Mutex::new(None)),
            event_handler: Arc::new(Mutex::new(None)),
            connection_state: Arc::new(Mutex::new(ConnectionState::Disconnected)),
        }
    }

    /* ─────────────────────────── Runtime helpers ─────────────────────────── */

    /// Manually set or clear the session token.
    pub async fn set_token(&self, token: Option<String>) {
        *self.token.lock().await = token;
    }

    /// Build an authenticated `reqwest::RequestBuilder`.
    async fn authed_request(
        &self,
        method: Method,
        url: &str,
        extra_headers: Option<&[(&str, &str)]>,
    ) -> reqwest::RequestBuilder {
        let token_opt = self.token.lock().await.clone();

        let mut req = self.http.request(method, url);

        // OS‑specific UA string.
        let user_agent = if cfg!(target_os = "windows") {
            WINDOWS_USER_AGENT
        } else {
            LINUX_USER_AGENT
        };

        req = req
            .header("User-Agent", user_agent)
            .header("Accept", "application/json, text/plain, */*")
            .header("Accept-Language", "en-US,en;q=0.5")
            .header("Content-Type", "application/json")
            .header("Referer", "https://revolt.onech.at/")
            .header("Origin", "https://revolt.onech.at")
            .header("DNT", "1")
            .header("Sec-GPC", "1")
            .header("Sec-Fetch-Dest", "empty")
            .header("Sec-Fetch-Mode", "cors")
            .header("Sec-Fetch-Site", "same-site")
            .header("Pragma", "no-cache")
            .header("Cache-Control", "no-cache")
            .header("TE", "trailers");

        if let Some(t) = token_opt {
            req = req.header("X-Session-Token", t);
        }

        if let Some(hdrs) = extra_headers {
            for (k, v) in hdrs {
                req = req.header(*k, *v);
            }
        }

        req
    }

    /* ───────────── Convenience wrappers around HTTP verbs ───────────── */

    pub async fn authed_get(
        &self,
        url: &str,
        extra_headers: Option<&[(&str, &str)]>,
    ) -> Result<Response, RevoltError> {
        self.authed_request(Method::GET, url, extra_headers)
            .await
            .send()
            .await
            .map_err(RevoltError::ReqwestError)
    }

    pub async fn authed_post<T: serde::Serialize>(
        &self,
        url: &str,
        body: &T,
        extra_headers: Option<&[(&str, &str)]>,
    ) -> Result<Response, RevoltError> {
        self.authed_request(Method::POST, url, extra_headers)
            .await
            .json(body)
            .send()
            .await
            .map_err(RevoltError::ReqwestError)
    }

    pub async fn authed_post_empty(
        &self,
        url: &str,
        extra_headers: Option<&[(&str, &str)]>,
    ) -> Result<Response, RevoltError> {
        self.authed_request(Method::POST, url, extra_headers)
            .await
            .send()
            .await
            .map_err(RevoltError::ReqwestError)
    }

    pub async fn authed_delete(
        &self,
        url: &str,
        extra_headers: Option<&[(&str, &str)]>,
    ) -> Result<Response, RevoltError> {
        self.authed_request(Method::DELETE, url, extra_headers)
            .await
            .send()
            .await
            .map_err(RevoltError::ReqwestError)
    }

    pub async fn authed_delete_with_query(
        &self,
        url: &str,
        query: &[(&str, String)],
        extra_headers: Option<&[(&str, &str)]>,
    ) -> Result<Response, RevoltError> {
        self.authed_request(Method::DELETE, url, extra_headers)
            .await
            .query(query)
            .send()
            .await
            .map_err(RevoltError::ReqwestError)
    }

    pub async fn authed_patch<T: serde::Serialize>(
        &self,
        url: &str,
        body: &T,
        extra_headers: Option<&[(&str, &str)]>,
    ) -> Result<Response, RevoltError> {
        self.authed_request(Method::PATCH, url, extra_headers)
            .await
            .json(body)
            .send()
            .await
            .map_err(RevoltError::ReqwestError)
    }
}

/// Parse the body as JSON **iff** the response status is success.
pub async fn parse_json_if_ok<T: serde::de::DeserializeOwned>(
    resp: Response,
) -> Result<T, RevoltError> {
    let status = resp.status();
    let bytes = resp.bytes().await.map_err(RevoltError::ReqwestError)?;

    if !status.is_success() {
        if let Ok(api_err) = serde_json::from_slice::<crate::types::error_types::Error>(&bytes) {
            return Err(handle_api_error(api_err));
        }

        return Err(RevoltError::HttpStatus {
            code: status.as_u16(),
            body: String::from_utf8_lossy(&bytes).to_string(),
        });
    }

    serde_json::from_slice::<T>(&bytes).map_err(RevoltError::SerdeError)
}
