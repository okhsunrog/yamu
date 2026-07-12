use std::{collections::BTreeMap, fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use url::Url;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LyricsFormat {
    #[default]
    Text,
    Lrc,
}

impl LyricsFormat {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Text => "TEXT",
            Self::Lrc => "LRC",
        }
    }

    pub const fn file_extension(self) -> &'static str {
        match self {
            Self::Text => "txt",
            Self::Lrc => "lrc",
        }
    }
}

impl fmt::Display for LyricsFormat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.file_extension())
    }
}

impl FromStr for LyricsFormat {
    type Err = ParseLyricsFormatError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "text" | "txt" => Ok(Self::Text),
            "lrc" => Ok(Self::Lrc),
            _ => Err(ParseLyricsFormatError(value.to_owned())),
        }
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
#[error("unsupported lyrics format {0:?}; expected text or lrc")]
pub struct ParseLyricsFormatError(String);

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LyricsMajor {
    pub id: Option<u64>,
    pub name: Option<String>,
    pub pretty_name: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TrackLyrics {
    pub download_url: Url,
    pub lyric_id: Option<u64>,
    pub external_lyric_id: Option<String>,
    #[serde(default)]
    pub writers: Vec<String>,
    pub major: Option<LyricsMajor>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}
