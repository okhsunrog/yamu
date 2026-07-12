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
#[cfg(feature = "lyrics")]
mod lyrics;
mod recommendations;
mod search;
mod track;

pub use account::{Account, AccountStatus};
pub use album::{Album, TrackPosition};
pub use artist::{Artist, ArtistAlbumSort, ArtistAlbumsPage, ArtistTracksPage, Cover};
#[cfg(feature = "oauth")]
pub use auth::{DeviceCode, OAuthToken};
pub use common::{Id, PageRequest};
#[cfg(feature = "downloads")]
pub use download::{
    AudioCodec, DownloadInfo, DownloadOptions, DownloadQuality, ParseDownloadQualityError,
};
pub use library::{
    LibraryRevision, Pager, Playlist, PlaylistDiff, PlaylistId, PlaylistOperation, PlaylistTrackId,
    PlaylistVisibility, TrackShort, TracksList, User,
};
#[cfg(feature = "lyrics")]
pub use lyrics::{LyricsFormat, LyricsMajor, ParseLyricsFormatError, TrackLyrics};
pub use recommendations::{
    PlaylistRecommendations, Station, StationDashboard, StationId, StationResult, StationSequence,
    StationTracks,
};
pub use search::{SearchPage, SearchResult};
pub use track::Track;
