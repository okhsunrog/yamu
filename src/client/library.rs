use serde::Deserialize;

use super::Client;
use crate::{
    Result,
    models::{Id, Playlist, Track, TracksList},
};

impl Client {
    /// Returns the revisioned liked-track library for a user.
    pub async fn liked_tracks(
        &self,
        user_id: impl Into<Id>,
        if_modified_since_revision: u64,
    ) -> Result<Option<TracksList>> {
        #[derive(serde::Serialize)]
        struct Query {
            #[serde(rename = "if-modified-since-revision")]
            revision: u64,
        }

        #[derive(Deserialize)]
        struct Response {
            library: Option<TracksList>,
        }

        let response: Response = self
            .get(
                &format!("users/{}/likes/tracks", user_id.into()),
                &Query {
                    revision: if_modified_since_revision,
                },
            )
            .await?;
        Ok(response.library)
    }

    /// Expands all compact entries from a revisioned track list.
    pub async fn tracks_from_list(&self, tracks: &TracksList) -> Result<Vec<Track>> {
        const BATCH_SIZE: usize = 100;

        let ids = tracks.track_ids().collect::<Vec<_>>();
        let mut expanded = Vec::with_capacity(ids.len());
        for batch in ids.chunks(BATCH_SIZE) {
            expanded.extend(self.tracks(batch.iter().map(String::as_str)).await?);
        }
        Ok(expanded)
    }

    /// Returns playlist summaries belonging to a user.
    pub async fn user_playlists(&self, user_id: impl Into<Id>) -> Result<Vec<Playlist>> {
        self.get(&format!("users/{}/playlists/list", user_id.into()), &())
            .await
    }

    /// Returns one playlist, including its compact track entries.
    pub async fn playlist(&self, owner_id: impl Into<Id>, kind: impl Into<Id>) -> Result<Playlist> {
        self.get(
            &format!("users/{}/playlists/{}", owner_id.into(), kind.into()),
            &(),
        )
        .await
    }
}
