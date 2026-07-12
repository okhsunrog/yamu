//! Shared local credential storage for Yandex Music applications.

mod error;
mod lifecycle;
mod record;
mod store;

pub use error::{Error, Result};
pub use lifecycle::{CredentialSource, RefreshPolicy, ResolvedCredentials};
pub use record::Credentials;
pub use store::{CredentialStore, DEFAULT_PROFILE, ProfileLock, TOKEN_ENV};
