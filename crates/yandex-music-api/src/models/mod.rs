//! Domain models returned by the API.

mod account;
mod album;
mod artist;
#[cfg(feature = "oauth")]
mod auth;
mod common;
#[cfg(feature = "downloads")]
mod download;
mod library;
mod search;
mod track;

pub use account::{Account, AccountStatus};
pub use album::{Album, TrackPosition};
pub use artist::{Artist, Cover};
#[cfg(feature = "oauth")]
pub use auth::{DeviceCode, OAuthToken};
pub use common::Id;
#[cfg(feature = "downloads")]
pub use download::{AudioCodec, DownloadInfo, DownloadOptions, DownloadQuality};
pub use library::{
    LibraryRevision, Pager, Playlist, PlaylistDiff, PlaylistId, PlaylistOperation, PlaylistTrackId,
    PlaylistVisibility, TrackShort, TracksList, User,
};
pub use search::{SearchPage, SearchResult};
pub use track::Track;
