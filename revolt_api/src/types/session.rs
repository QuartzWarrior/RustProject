use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    #[serde(rename = "_id")]
    pub id: String,
    pub name: String,
}

/// Used to edit a session (like the friendly name).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataEditSession {
    pub friendly_name: String,
}
