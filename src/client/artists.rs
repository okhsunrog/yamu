use reqwest::Method;
use serde::Serialize;

use super::Client;
use crate::{
    Result,
    models::{Artist, ArtistAlbumSort, ArtistAlbumsPage, ArtistTracksPage, Id, PageRequest},
};

impl Client {
    /// Fetches one or more artists in a single batch request.
    pub async fn artists<I, T>(&self, ids: I) -> Result<Vec<Artist>>
    where
        I: IntoIterator<Item = T>,
        T: Into<Id>,
    {
        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        for id in ids {
            serializer.append_pair("artist-ids", &id.into().to_string());
        }
        let request = self
            .request(Method::POST, "artists")?
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(serializer.finish());
        self.send(request).await
    }

    pub async fn artist_tracks(
        &self,
        artist_id: impl Into<Id>,
        page: PageRequest,
    ) -> Result<ArtistTracksPage> {
        #[derive(Serialize)]
        struct Query {
            page: u32,
            #[serde(rename = "page-size")]
            page_size: u32,
        }
        self.get(
            &format!("artists/{}/tracks", artist_id.into()),
            &Query {
                page: page.page(),
                page_size: page.page_size(),
            },
        )
        .await
    }

    pub async fn all_artist_tracks(
        &self,
        artist_id: impl Into<Id>,
    ) -> Result<Vec<crate::models::Track>> {
        let artist_id = artist_id.into();
        let mut request = PageRequest::default();
        let mut tracks = Vec::new();
        loop {
            let page = self.artist_tracks(artist_id.clone(), request).await?;
            let count = page.tracks.len();
            let total = page.pager.as_ref().and_then(|pager| pager.total);
            tracks.extend(page.tracks);
            if count < request.page_size() as usize
                || total.is_some_and(|total| tracks.len() as u64 >= total)
            {
                return Ok(tracks);
            }
            request = request.next();
        }
    }

    pub async fn artist_albums(
        &self,
        artist_id: impl Into<Id>,
        page: PageRequest,
        sort: ArtistAlbumSort,
    ) -> Result<ArtistAlbumsPage> {
        #[derive(Serialize)]
        struct Query<'a> {
            page: u32,
            #[serde(rename = "page-size")]
            page_size: u32,
            #[serde(rename = "sort-by")]
            sort_by: &'a str,
        }
        self.get(
            &format!("artists/{}/direct-albums", artist_id.into()),
            &Query {
                page: page.page(),
                page_size: page.page_size(),
                sort_by: sort.as_str(),
            },
        )
        .await
    }

    pub async fn all_artist_albums(
        &self,
        artist_id: impl Into<Id>,
        sort: ArtistAlbumSort,
    ) -> Result<Vec<crate::models::Album>> {
        let artist_id = artist_id.into();
        let mut request = PageRequest::default();
        let mut albums = Vec::new();
        loop {
            let page = self.artist_albums(artist_id.clone(), request, sort).await?;
            let count = page.albums.len();
            let total = page.pager.as_ref().and_then(|pager| pager.total);
            albums.extend(page.albums);
            if count < request.page_size() as usize
                || total.is_some_and(|total| albums.len() as u64 >= total)
            {
                return Ok(albums);
            }
            request = request.next();
        }
    }
}
