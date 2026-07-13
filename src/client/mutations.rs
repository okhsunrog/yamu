use reqwest::Method;
use serde::Serialize;

use super::Client;
use crate::{
    Error, Result,
    models::{Id, LibraryRevision, Playlist, PlaylistDiff, PlaylistVisibility},
};

impl Client {
    /// Adds tracks to a user's liked-track library and returns its new revision.
    pub async fn like_tracks<I, T>(
        &self,
        user_id: impl Into<Id>,
        track_ids: I,
    ) -> Result<LibraryRevision>
    where
        I: IntoIterator<Item = T>,
        T: Into<Id>,
    {
        self.change_track_likes(user_id.into(), track_ids, "add-multiple")
            .await
    }

    /// Removes tracks from a user's liked-track library and returns its new revision.
    pub async fn unlike_tracks<I, T>(
        &self,
        user_id: impl Into<Id>,
        track_ids: I,
    ) -> Result<LibraryRevision>
    where
        I: IntoIterator<Item = T>,
        T: Into<Id>,
    {
        self.change_track_likes(user_id.into(), track_ids, "remove")
            .await
    }

    /// Creates a playlist owned by `user_id`.
    pub async fn create_playlist(
        &self,
        user_id: impl Into<Id>,
        title: impl AsRef<str>,
        visibility: PlaylistVisibility,
    ) -> Result<Playlist> {
        #[derive(Serialize)]
        struct Form<'a> {
            title: &'a str,
            visibility: PlaylistVisibility,
        }

        let user_id = user_id.into().to_string();
        let request = self
            .request_segments(
                Method::POST,
                ["users", user_id.as_str(), "playlists", "create"],
            )?
            .form(&Form {
                title: title.as_ref(),
                visibility,
            });
        self.send(request).await
    }

    /// Deletes a playlist owned by `user_id`.
    pub async fn delete_playlist(&self, user_id: impl Into<Id>, kind: impl Into<Id>) -> Result<()> {
        let user_id = user_id.into().to_string();
        let kind = kind.into().to_string();
        let request = self.request_segments(
            Method::POST,
            [
                "users",
                user_id.as_str(),
                "playlists",
                kind.as_str(),
                "delete",
            ],
        )?;
        let result: String = self.send(request).await?;
        if result == "ok" {
            Ok(())
        } else {
            Err(Error::InvalidResponse {
                message: format!("playlist deletion returned {result:?} instead of `ok`"),
                source: None,
            })
        }
    }

    /// Renames a playlist owned by `user_id`.
    pub async fn rename_playlist(
        &self,
        user_id: impl Into<Id>,
        kind: impl Into<Id>,
        title: impl AsRef<str>,
    ) -> Result<Playlist> {
        self.set_playlist_value(user_id.into(), kind.into(), "name", title.as_ref())
            .await
    }

    /// Changes the visibility of a playlist owned by `user_id`.
    pub async fn set_playlist_visibility(
        &self,
        user_id: impl Into<Id>,
        kind: impl Into<Id>,
        visibility: PlaylistVisibility,
    ) -> Result<Playlist> {
        self.set_playlist_value(
            user_id.into(),
            kind.into(),
            "visibility",
            &visibility.to_string(),
        )
        .await
    }

    /// Applies `diff` only if `revision` is still current.
    ///
    /// A stale revision is returned as [`Error::PlaylistRevisionConflict`]. The
    /// operation is never retried automatically because blindly replaying a
    /// positional diff against a newer playlist could alter the wrong tracks.
    pub async fn change_playlist(
        &self,
        user_id: impl Into<Id>,
        kind: impl Into<Id>,
        revision: u64,
        diff: &PlaylistDiff,
    ) -> Result<Playlist> {
        #[derive(Serialize)]
        struct Form<'a> {
            kind: &'a str,
            revision: u64,
            diff: String,
        }

        let user_id = user_id.into().to_string();
        let kind = kind.into().to_string();
        let request = self
            .request_segments(
                Method::POST,
                [
                    "users",
                    user_id.as_str(),
                    "playlists",
                    kind.as_str(),
                    "change",
                ],
            )?
            .form(&Form {
                kind: &kind,
                revision,
                diff: serde_json::to_string(diff)?,
            });

        match self.send(request).await {
            Err(Error::Api {
                status,
                message,
                body,
            }) if status == reqwest::StatusCode::CONFLICT => Err(Error::PlaylistRevisionConflict {
                expected_revision: revision,
                message,
                body,
            }),
            result => result,
        }
    }

    async fn change_track_likes<I, T>(
        &self,
        user_id: Id,
        track_ids: I,
        action: &str,
    ) -> Result<LibraryRevision>
    where
        I: IntoIterator<Item = T>,
        T: Into<Id>,
    {
        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        for track_id in track_ids {
            serializer.append_pair("track-ids", &track_id.into().to_string());
        }
        let user_id = user_id.to_string();
        let request = self
            .request_segments(
                Method::POST,
                ["users", user_id.as_str(), "likes", "tracks", action],
            )?
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(serializer.finish());
        self.send(request).await
    }

    async fn set_playlist_value(
        &self,
        user_id: Id,
        kind: Id,
        field: &str,
        value: &str,
    ) -> Result<Playlist> {
        let user_id = user_id.to_string();
        let kind = kind.to_string();
        let request = self
            .request_segments(
                Method::POST,
                ["users", user_id.as_str(), "playlists", kind.as_str(), field],
            )?
            .form(&[("value", value)]);
        self.send(request).await
    }
}
