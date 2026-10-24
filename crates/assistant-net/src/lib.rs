//! The app's network layer: an HTTPS client that can reach only the two
//! providers' APIs, and their clients.

#![forbid(unsafe_code)]

pub mod anthropic;
mod base64;
pub mod http;
pub mod openrouter;
pub mod provider;
pub mod retry;
pub mod roots;

pub use http::{Client, HOSTS, Host, NetError};
pub use provider::{Citation, Effort, Part, Picodollars, Schema};
