//! Unofficial, asynchronous Rust client for the Yandex Music API.
//!
//! The API is not publicly documented and can change without notice. This
//! crate keeps its models forward-compatible by retaining unknown JSON fields.

#[cfg(any(
    feature = "credentials",
    feature = "downloader",
    feature = "media-ffmpeg-cli",
    feature = "media-ffmpeg"
))]
mod atomic_file;
#[cfg(feature = "oauth")]
pub mod auth;
mod client;
#[cfg(feature = "credentials")]
pub mod credentials;
#[cfg(feature = "downloader")]
pub mod downloader;
mod error;
#[cfg(feature = "media")]
pub mod media;
pub mod models;
pub mod resource;
#[cfg(any(
    feature = "downloader",
    feature = "media-ffmpeg-cli",
    feature = "media-ffmpeg"
))]
mod temporary_file;

pub use client::{Client, ClientBuilder, ReadRequestPolicy, SearchOptions, SearchType};
pub use error::{Error, Result};
