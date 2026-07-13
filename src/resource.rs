//! Typed references to Yandex Music resources.

use std::{fmt, str::FromStr};

use thiserror::Error;
use url::Url;

/// A track reference parsed from an ID or a Yandex Music URL.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TrackRef {
    track_id: String,
    album_id: Option<String>,
}

impl TrackRef {
    pub fn new(track_id: impl Into<String>) -> Result<Self, ParseResourceRefError> {
        Ok(Self {
            track_id: validate_id("track", track_id.into())?,
            album_id: None,
        })
    }

    pub fn track_id(&self) -> &str {
        &self.track_id
    }

    pub fn album_id(&self) -> Option<&str> {
        self.album_id.as_deref()
    }

    pub fn canonical_url(&self) -> Url {
        let path = match &self.album_id {
            Some(album_id) => format!("album/{album_id}/track/{}", self.track_id),
            None => format!("track/{}", self.track_id),
        };
        music_url(&path)
    }
}

impl fmt::Display for TrackRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.track_id)
    }
}

impl FromStr for TrackRef {
    type Err = ParseResourceRefError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if let Some(segments) = music_url_segments(value)? {
            let segments = segments.iter().map(String::as_str).collect::<Vec<_>>();
            return match segments.as_slice() {
                ["album", album_id, "track", track_id] | ["track", track_id, "album", album_id] => {
                    Ok(Self {
                        track_id: validate_id("track", (*track_id).to_owned())?,
                        album_id: Some(validate_id("album", (*album_id).to_owned())?),
                    })
                }
                ["track", track_id] => Self::new(*track_id),
                _ => Err(ParseResourceRefError::WrongResource {
                    expected: "track",
                    value: value.to_owned(),
                }),
            };
        }
        Self::new(value.trim())
    }
}

macro_rules! simple_resource_ref {
    ($name:ident, $kind:literal, $segment:literal, $accessor:ident) => {
        #[doc = concat!("A ", $kind, " reference parsed from an ID or a Yandex Music URL.")]
        #[derive(Clone, Debug, PartialEq, Eq, Hash)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, ParseResourceRefError> {
                Ok(Self(validate_id($kind, value.into())?))
            }

            pub fn $accessor(&self) -> &str {
                &self.0
            }

            pub fn canonical_url(&self) -> Url {
                music_url(&format!(concat!($segment, "/{}"), self.0))
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl FromStr for $name {
            type Err = ParseResourceRefError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                if let Some(segments) = music_url_segments(value)? {
                    let segments = segments.iter().map(String::as_str).collect::<Vec<_>>();
                    return match segments.as_slice() {
                        [$segment, id] => Self::new(*id),
                        _ => Err(ParseResourceRefError::WrongResource {
                            expected: $kind,
                            value: value.to_owned(),
                        }),
                    };
                }
                Self::new(value.trim())
            }
        }
    };
}

simple_resource_ref!(AlbumRef, "album", "album", album_id);
simple_resource_ref!(ArtistRef, "artist", "artist", artist_id);

/// A playlist reference parsed from `owner:kind` or a Yandex Music URL.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PlaylistRef {
    owner: String,
    kind: String,
}

impl PlaylistRef {
    pub fn new(
        owner: impl Into<String>,
        kind: impl Into<String>,
    ) -> Result<Self, ParseResourceRefError> {
        Ok(Self {
            owner: validate_id("playlist owner", owner.into())?,
            kind: validate_id("playlist kind", kind.into())?,
        })
    }

    pub fn owner(&self) -> &str {
        &self.owner
    }

    pub fn kind(&self) -> &str {
        &self.kind
    }

    pub fn canonical_url(&self) -> Url {
        music_url(&format!("users/{}/playlists/{}", self.owner, self.kind))
    }
}

impl fmt::Display for PlaylistRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}", self.owner, self.kind)
    }
}

impl FromStr for PlaylistRef {
    type Err = ParseResourceRefError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if let Some(segments) = music_url_segments(value)? {
            let segments = segments.iter().map(String::as_str).collect::<Vec<_>>();
            return match segments.as_slice() {
                ["users", owner, "playlists", kind] => Self::new(*owner, *kind),
                _ => Err(ParseResourceRefError::WrongResource {
                    expected: "playlist",
                    value: value.to_owned(),
                }),
            };
        }
        let (owner, kind) = value
            .trim()
            .split_once(':')
            .ok_or_else(|| ParseResourceRefError::InvalidPlaylist(value.to_owned()))?;
        Self::new(owner, kind)
    }
}

/// Error returned when a resource ID or URL cannot be interpreted.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ParseResourceRefError {
    #[error("invalid Yandex Music URL {0:?}")]
    InvalidUrl(String),
    #[error("URL {value:?} points to a different resource; expected {expected}")]
    WrongResource {
        expected: &'static str,
        value: String,
    },
    #[error("empty {0} identifier")]
    EmptyId(&'static str),
    #[error("invalid {kind} identifier {value:?}")]
    InvalidId { kind: &'static str, value: String },
    #[error("invalid playlist reference {0:?}; expected owner:kind or a playlist URL")]
    InvalidPlaylist(String),
}

fn validate_id(kind: &'static str, value: String) -> Result<String, ParseResourceRefError> {
    let value = value.trim();
    if value.is_empty() {
        Err(ParseResourceRefError::EmptyId(kind))
    } else if matches!(value, "." | "..") || value.contains(['/', '?', '#']) {
        Err(ParseResourceRefError::InvalidId {
            kind,
            value: value.to_owned(),
        })
    } else {
        Ok(value.to_owned())
    }
}

fn music_url_segments(value: &str) -> Result<Option<Vec<String>>, ParseResourceRefError> {
    let value = value.trim();
    if !value.contains("://") {
        return Ok(None);
    }
    let url = Url::parse(value).map_err(|_| ParseResourceRefError::InvalidUrl(value.to_owned()))?;
    let valid_host = url.host_str().is_some_and(|host| {
        host == "music.yandex.ru"
            || host
                .strip_prefix("music.yandex.")
                .is_some_and(|tld| !tld.is_empty() && !tld.contains('.'))
    });
    if url.scheme() != "https" || !valid_host {
        return Err(ParseResourceRefError::InvalidUrl(value.to_owned()));
    }
    Ok(Some(
        url.path_segments()
            .ok_or_else(|| ParseResourceRefError::InvalidUrl(value.to_owned()))?
            .filter(|segment| !segment.is_empty())
            .map(str::to_owned)
            .collect(),
    ))
}

fn music_url(path: &str) -> Url {
    Url::parse(&format!("https://music.yandex.ru/{path}"))
        .expect("resource IDs were validated for URL path use")
}
