use super::Client;
use crate::{
    Result,
    models::{Album, Id},
};

impl Client {
    /// Fetches an album together with tracks grouped by volume.
    pub async fn album_with_tracks(&self, album_id: impl Into<Id>) -> Result<Album> {
        self.get(&format!("albums/{}/with-tracks", album_id.into()), &())
            .await
    }
}
