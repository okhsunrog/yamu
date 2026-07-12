//! Shared local credential storage for Yandex Music applications.

mod credentials;
mod error;
mod lifecycle;
mod store;

pub use credentials::Credentials;
pub use error::{Error, Result};
pub use lifecycle::{CredentialSource, RefreshPolicy, ResolvedCredentials};
pub use store::{CredentialStore, DEFAULT_PROFILE, ProfileLock, TOKEN_ENV};
