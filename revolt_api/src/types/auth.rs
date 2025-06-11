use serde::{Deserialize, Serialize};

/// DataLogin is a oneOf with:
/// - { email, password, friendly_name? } 
/// - or { mfa_ticket, mfa_response?, friendly_name? }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DataLogin {
    Plain {
        email: String,
        password: String,
        friendly_name: Option<String>,
    },
    MfaTicket {
        mfa_ticket: String,
        mfa_response: Option<MFAResponse>,
        friendly_name: Option<String>,
    },
}

/// Possibly returned after login:
/// 1) Success { _id, user_id, token, name, subscription? }
/// 2) MFA { allowed_methods, ticket }
/// 3) Disabled { user_id }
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ResponseLogin {
    Success {
        #[serde(rename = "_id")]
        id: String,
        user_id: String,
        token: String,
        name: String,
        subscription: Option<WebPushSubscription>,
        result: String,
    },
    MfaChallenge {
        result: String,
        ticket: String,
        allowed_methods: Vec<MFAMethod>,
    },
    Disabled {
        user_id: String,
        result: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MFAResponse {
    // Implementation depends on the type of MFA. This is just a placeholder field.
    pub code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebPushSubscription {
    // A placeholder, define fields if needed
    pub endpoint: String,
    pub p256dh: String,
    pub auth: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MFAMethod {
    // Example: "totp", "recovery"
    pub r#type: String,
    // Possibly more fields
}
