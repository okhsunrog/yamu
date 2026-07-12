use reqwest::Method;

use super::Client;
use crate::{
    Result,
    models::{Id, Track},
};

impl Client {
    /// Fetches one or more tracks in a single batch request.
    pub async fn tracks<I, T>(&self, ids: I) -> Result<Vec<Track>>
    where
        I: IntoIterator<Item = T>,
        T: Into<Id>,
    {
        let ids: Vec<String> = ids.into_iter().map(|id| id.into().to_string()).collect();
        if ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        for id in ids {
            serializer.append_pair("track-ids", &id);
        }

        let request = self
            .request(Method::POST, "tracks")?
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(serializer.finish());
        self.send(request).await
    }
}
