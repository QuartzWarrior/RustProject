use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)] // Use default values for missing fields
pub struct User {
    #[serde(rename = "_id")]
    pub id: String,
    pub username: String,
    pub discriminator: String,
    pub display_name: Option<String>,
    pub avatar: Option<super::message::File>,
    pub relations: Option<Vec<Relationship>>,
    pub badges: Option<u32>,
    pub status: Option<UserStatus>,
    pub flags: Option<u32>,
    pub privileged: bool,
    pub bot: Option<BotInformation>,
    pub relationship: RelationshipStatus,
    pub online: bool,
}

impl Default for User {
    fn default() -> Self {
        Self {
            id: String::new(),
            username: String::new(),
            discriminator: String::new(),
            display_name: None,
            avatar: None,
            relations: None,
            badges: None,
            status: None,
            flags: None,
            privileged: false,
            bot: None,
            relationship: RelationshipStatus::None,
            online: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Relationship {
    #[serde(rename = "_id")]
    pub id: String,
    pub status: RelationshipStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum RelationshipStatus {
    None,
    User,
    Friend,
    Outgoing,
    Incoming,
    Blocked,
    BlockedOther,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserStatus {
    pub text: Option<String>,
    pub presence: Option<Presence>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum Presence {
    Online,
    Idle,
    Focus,
    Busy,
    Invisible,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotInformation {
    pub owner: String,
}

/// A server Member
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Member {
    #[serde(rename = "_id")]
    pub id: MemberCompositeKey,
    pub joined_at: String, // ISO8601
    pub nickname: Option<String>,
    pub avatar: Option<super::message::File>,
    pub roles: Option<Vec<String>>,
    pub timeout: Option<String>, // ISO8601
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemberCompositeKey {
    pub server: String,
    pub user: String,
}
