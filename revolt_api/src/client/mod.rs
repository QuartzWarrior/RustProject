mod client;

pub use client::RevoltClient; 
pub use client::parse_json_if_ok;

pub use crate::{
    api::{auth::AuthApi, messages::MessagesApi, session::SessionApi},
    error::{handle_api_error, RevoltError},
};