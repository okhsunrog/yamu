use serde::Serialize;

use super::Client;
use crate::{Result, models::SearchResult};

impl Client {
    /// Searches across all supported entity types using default options.
    pub async fn search(&self, text: &str) -> Result<SearchResult> {
        self.search_with_options(text, &SearchOptions::default())
            .await
    }

    /// Searches with explicit type, page and correction options.
    pub async fn search_with_options(
        &self,
        text: &str,
        options: &SearchOptions,
    ) -> Result<SearchResult> {
        #[derive(Serialize)]
        struct Query<'a> {
            text: &'a str,
            nocorrect: bool,
            #[serde(rename = "type")]
            kind: SearchType,
            page: u32,
            #[serde(rename = "playlist-in-best")]
            playlist_in_best: bool,
        }

        self.get(
            "search",
            &Query {
                text,
                nocorrect: options.no_correct,
                kind: options.kind,
                page: options.page,
                playlist_in_best: options.playlist_in_best,
            },
        )
        .await
    }
}

/// Entity category used by search.
#[derive(Clone, Copy, Debug, Default, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SearchType {
    #[default]
    All,
    Artist,
    User,
    Album,
    Playlist,
    Track,
    Podcast,
    PodcastEpisode,
}

/// Optional parameters accepted by [`Client::search_with_options`].
#[derive(Clone, Debug)]
pub struct SearchOptions {
    pub no_correct: bool,
    pub kind: SearchType,
    pub page: u32,
    pub playlist_in_best: bool,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            no_correct: false,
            kind: SearchType::All,
            page: 0,
            playlist_in_best: true,
        }
    }
}
