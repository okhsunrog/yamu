//! Domain models returned by the API.

mod account;
mod album;
mod artist;
mod auth;
mod common;
mod library;
mod search;
mod track;

pub use account::{Account, AccountStatus};
pub use album::Album;
pub use artist::{Artist, Cover};
pub use auth::{DeviceCode, OAuthToken};
pub use common::Id;
pub use library::{
    LibraryRevision, Pager, Playlist, PlaylistDiff, PlaylistId, PlaylistOperation, PlaylistTrackId,
    PlaylistVisibility, TrackShort, TracksList, User,
};
pub use search::{SearchPage, SearchResult};
pub use track::Track;
