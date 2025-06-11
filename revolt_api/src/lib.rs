//! # Revolt API
//!
//! This library provides an asynchronous, object-oriented Rust client for
//! interacting with a Revolt-style API. It uses `tokio` for async runtime
//! and `reqwest` for HTTP requests. It maps the provided OpenAPI schema
//! into a set of Rust data structures and endpoint methods.

pub mod client;
pub mod error;
pub mod util;
pub mod api;
pub mod websocket;
pub mod types;

pub use client::*;
pub use types::*;
pub use websocket::*;
pub use error::RevoltError;
