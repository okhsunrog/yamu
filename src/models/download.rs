use std::{collections::BTreeMap, fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use serde_json::Value;
use thiserror::Error;
use url::Url;

/// Requested audio quality. The server may return a lower available quality.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DownloadQuality {
    Low,
    Normal,
    #[default]
    Lossless,
}

impl DownloadQuality {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Low => "lq",
            Self::Normal => "nq",
            Self::Lossless => "lossless",
        }
    }
}

impl fmt::Display for DownloadQuality {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for DownloadQuality {
    type Err = ParseDownloadQualityError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "low" | "lq" => Ok(Self::Low),
            "normal" | "nq" => Ok(Self::Normal),
            "lossless" => Ok(Self::Lossless),
            _ => Err(ParseDownloadQualityError(value.to_owned())),
        }
    }
}

/// Error returned when parsing an unsupported requested audio quality.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
#[error("unsupported download quality {0:?}; expected low, normal, or lossless")]
pub struct ParseDownloadQualityError(String);

/// Audio codec/container identifier understood by the file-info endpoint.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AudioCodec {
    Flac,
    Aac,
    HeAac,
    Mp3,
    FlacMp4,
    AacMp4,
    HeAacMp4,
    Other(String),
}

impl AudioCodec {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Flac => "flac",
            Self::Aac => "aac",
            Self::HeAac => "he-aac",
            Self::Mp3 => "mp3",
            Self::FlacMp4 => "flac-mp4",
            Self::AacMp4 => "aac-mp4",
            Self::HeAacMp4 => "he-aac-mp4",
            Self::Other(value) => value,
        }
    }

    /// Conventional extension for files returned with this codec identifier.
    pub fn file_extension(&self) -> &str {
        match self {
            Self::Flac => "flac",
            Self::Mp3 => "mp3",
            Self::Aac | Self::HeAac | Self::FlacMp4 | Self::AacMp4 | Self::HeAacMp4 => "m4a",
            Self::Other(_) => "bin",
        }
    }
}

impl fmt::Display for AudioCodec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for AudioCodec {
    type Err = std::convert::Infallible;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(match value {
            "flac" => Self::Flac,
            "aac" => Self::Aac,
            "he-aac" => Self::HeAac,
            "mp3" => Self::Mp3,
            "flac-mp4" => Self::FlacMp4,
            "aac-mp4" => Self::AacMp4,
            "he-aac-mp4" => Self::HeAacMp4,
            other => Self::Other(other.to_owned()),
        })
    }
}

impl Serialize for AudioCodec {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for AudioCodec {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_str(&value).map_err(D::Error::custom)
    }
}

/// Negotiation parameters for [`crate::Client::download_info`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DownloadOptions {
    pub quality: DownloadQuality,
    pub codecs: Vec<AudioCodec>,
}

impl Default for DownloadOptions {
    fn default() -> Self {
        Self {
            quality: DownloadQuality::Lossless,
            codecs: vec![
                AudioCodec::Flac,
                AudioCodec::Aac,
                AudioCodec::HeAac,
                AudioCodec::Mp3,
                AudioCodec::FlacMp4,
                AudioCodec::AacMp4,
                AudioCodec::HeAacMp4,
            ],
        }
    }
}

/// Negotiated audio source returned by Yandex Music.
#[derive(Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DownloadInfo {
    pub quality: String,
    pub codec: AudioCodec,
    pub bitrate: u32,
    #[serde(default)]
    pub urls: Vec<Url>,
    #[serde(default, rename = "key", skip_serializing)]
    pub decryption_key: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl fmt::Debug for DownloadInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DownloadInfo")
            .field("quality", &self.quality)
            .field("codec", &self.codec)
            .field("bitrate", &self.bitrate)
            .field("url_count", &self.urls.len())
            .field("encrypted", &self.decryption_key.is_some())
            .field("extra", &self.extra)
            .finish()
    }
}
