use serde::Serialize;

use super::Client;
use crate::{
    Result,
    models::{
        Id, PlaylistRecommendations, StationDashboard, StationId, StationResult, StationTracks,
    },
};

impl Client {
    pub async fn playlist_recommendations(
        &self,
        owner_id: impl Into<Id>,
        kind: impl Into<Id>,
    ) -> Result<PlaylistRecommendations> {
        self.get(
            &format!(
                "users/{}/playlists/{}/recommendations",
                owner_id.into(),
                kind.into()
            ),
            &(),
        )
        .await
    }

    pub async fn stations_dashboard(&self) -> Result<StationDashboard> {
        self.get("rotor/stations/dashboard", &()).await
    }

    pub async fn stations(&self, language: impl AsRef<str>) -> Result<Vec<StationResult>> {
        #[derive(Serialize)]
        struct Query<'a> {
            language: &'a str,
        }
        self.get(
            "rotor/stations/list",
            &Query {
                language: language.as_ref(),
            },
        )
        .await
    }

    pub async fn station_info(&self, station: &StationId) -> Result<Vec<StationResult>> {
        self.get(&format!("rotor/station/{station}/info"), &())
            .await
    }

    pub async fn station_tracks(
        &self,
        station: &StationId,
        queue: Option<Id>,
    ) -> Result<StationTracks> {
        #[derive(Serialize)]
        struct Query {
            settings2: bool,
            #[serde(skip_serializing_if = "Option::is_none")]
            queue: Option<String>,
        }
        self.get(
            &format!("rotor/station/{station}/tracks"),
            &Query {
                settings2: true,
                queue: queue.map(|id| id.to_string()),
            },
        )
        .await
    }
}
