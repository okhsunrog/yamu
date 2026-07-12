//! Shared local credential storage for Yandex Music applications.

mod credentials;
mod error;
mod store;

pub use credentials::Credentials;
pub use error::{Error, Result};
pub use store::{CredentialStore, DEFAULT_PROFILE, TOKEN_ENV};
