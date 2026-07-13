use super::Client;
use crate::{
    Result,
    models::{Album, Id},
};

impl Client {
    /// Fetches an album together with tracks grouped by volume.
    pub async fn album_with_tracks(&self, album_id: impl Into<Id>) -> Result<Album> {
        let album_id = album_id.into().to_string();
        self.get_segments(["albums", album_id.as_str(), "with-tracks"], &())
            .await
    }
}
