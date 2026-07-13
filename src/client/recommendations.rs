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
        let owner_id = owner_id.into().to_string();
        let kind = kind.into().to_string();
        self.get_segments(
            [
                "users",
                owner_id.as_str(),
                "playlists",
                kind.as_str(),
                "recommendations",
            ],
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
        let station = station.to_string();
        self.get_segments(["rotor", "station", station.as_str(), "info"], &())
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
        let station = station.to_string();
        self.get_segments(
            ["rotor", "station", station.as_str(), "tracks"],
            &Query {
                settings2: true,
                queue: queue.map(|id| id.to_string()),
            },
        )
        .await
    }
}
