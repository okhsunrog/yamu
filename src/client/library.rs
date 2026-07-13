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

        let user_id = user_id.into().to_string();
        let response: Response = self
            .get_segments(
                ["users", user_id.as_str(), "likes", "tracks"],
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
        let user_id = user_id.into().to_string();
        self.get_segments(["users", user_id.as_str(), "playlists", "list"], &())
            .await
    }

    /// Returns one playlist, including its compact track entries.
    pub async fn playlist(&self, owner_id: impl Into<Id>, kind: impl Into<Id>) -> Result<Playlist> {
        let owner_id = owner_id.into().to_string();
        let kind = kind.into().to_string();
        self.get_segments(
            ["users", owner_id.as_str(), "playlists", kind.as_str()],
            &(),
        )
        .await
    }
}
