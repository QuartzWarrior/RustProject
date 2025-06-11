use serde::{Deserialize, Serialize};

/// The top-level error object with a location and a typed variant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Error {
    pub location: String,
    #[serde(flatten)]
    pub kind: ErrorKind,
}

/// We flatten the "oneOf" into an enum with possible fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
#[serde(rename_all = "PascalCase")]
pub enum ErrorKind {
    LabelMe,
    AlreadyOnboarded,
    UsernameTaken,
    InvalidUsername,
    DiscriminatorChangeRatelimited,
    UnknownUser,
    AlreadyFriends,
    AlreadySentRequest,
    Blocked,
    BlockedByOther,
    NotFriends,
    TooManyPendingFriendRequests { max: u32 },
    UnknownChannel,
    UnknownAttachment,
    UnknownMessage,
    CannotEditMessage,
    CannotJoinCall,
    TooManyAttachments { max: u32 },
    TooManyEmbeds { max: u32 },
    TooManyReplies { max: u32 },
    TooManyChannels { max: u32 },
    EmptyMessage,
    PayloadTooLarge,
    CannotRemoveYourself,
    GroupTooLarge { max: u32 },
    AlreadyInGroup,
    NotInGroup,
    UnknownServer,
    InvalidRole,
    Banned,
    TooManyServers { max: u32 },
    TooManyEmoji { max: u32 },
    TooManyRoles { max: u32 },
    AlreadyInServer,
    ReachedMaximumBots,
    IsBot,
    BotIsPrivate,
    CannotReportYourself,
    MissingPermission { permission: String },
    MissingUserPermission { permission: String },
    NotElevated,
    NotPrivileged,
    CannotGiveMissingPermissions,
    NotOwner,
    DatabaseError { operation: String, collection: String },
    InternalError,
    InvalidOperation,
    InvalidCredentials,
    InvalidProperty,
    InvalidSession,
    DuplicateNonce,
    NotFound,
    NoEffect,
    FailedValidation { error: String },
    VosoUnavailable,
}
