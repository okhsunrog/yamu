//! Unofficial, asynchronous Rust client for the Yandex Music API.
//!
//! The API is not publicly documented and can change without notice. This
//! crate keeps its models forward-compatible by retaining unknown JSON fields.

#[cfg(feature = "oauth")]
pub mod auth;
mod client;
#[cfg(feature = "credentials")]
pub mod credentials;
mod error;
pub mod models;
pub mod resource;

pub use client::{Client, ClientBuilder, ReadRequestPolicy, SearchOptions, SearchType};
pub use error::{Error, Result};
