use std::time::{SystemTime, UNIX_EPOCH};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use hmac::{Hmac, KeyInit, Mac};
use serde::Serialize;
use sha2::Sha256;

use super::Client;
use crate::{
    Error, Result,
    models::{Id, LyricsFormat, TrackLyrics},
};

const LYRICS_SIGN_KEY: &[u8] = b"p93jhgh689SBReK6ghtw62";

impl Client {
    pub async fn track_lyrics(
        &self,
        track_id: impl Into<Id>,
        format: LyricsFormat,
    ) -> Result<TrackLyrics> {
        let raw_track_id = track_id.into().to_string();
        let numeric_track_id = raw_track_id
            .split(':')
            .next()
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or_else(|| Error::InvalidLyricsTrackId(raw_track_id.clone()))?;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| Error::InvalidSystemClock)?
            .as_secs();
        let sign = sign_lyrics(numeric_track_id, timestamp);

        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Query<'a> {
            format: &'a str,
            time_stamp: u64,
            sign: &'a str,
        }
        self.get(
            &format!("tracks/{numeric_track_id}/lyrics"),
            &Query {
                format: format.as_str(),
                time_stamp: timestamp,
                sign: &sign,
            },
        )
        .await
    }

    /// Fetches lyric text without forwarding the OAuth token to object storage.
    pub async fn fetch_lyrics(&self, lyrics: &TrackLyrics) -> Result<String> {
        Ok(self
            .http
            .get(lyrics.download_url.clone())
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?)
    }
}

fn sign_lyrics(track_id: u64, timestamp: u64) -> String {
    let message = format!("{track_id}{timestamp}");
    let mut mac =
        Hmac::<Sha256>::new_from_slice(LYRICS_SIGN_KEY).expect("HMAC accepts any key length");
    mac.update(message.as_bytes());
    STANDARD.encode(mac.finalize().into_bytes())
}

#[cfg(test)]
mod tests {
    use super::sign_lyrics;

    #[test]
    fn signs_known_lyrics_request() {
        assert_eq!(
            sign_lyrics(4_784_420, 1_668_687_184),
            "xFMznx4AQv6pjJr9z1NUQFADPasQt+USWFjBOowbpbU="
        );
    }
}
