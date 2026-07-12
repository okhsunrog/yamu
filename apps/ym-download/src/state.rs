use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::{io::AsyncWriteExt, sync::Mutex};

use crate::PlaylistJob;

const STATE_VERSION: u32 = 1;
const STATE_FILE: &str = ".ym-download-state.json";

#[derive(Clone)]
pub struct PlaylistStateStore {
    path: PathBuf,
    state: Arc<Mutex<PlaylistState>>,
    write_lock: Arc<Mutex<()>>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StateStatus {
    Pending,
    Downloaded,
    Verified,
    Repaired,
    Failed,
}

#[derive(Debug, Deserialize, Serialize)]
struct PlaylistState {
    version: u32,
    owner: String,
    kind: String,
    updated_at: u64,
    entries: BTreeMap<usize, StateEntry>,
}

#[derive(Debug, Deserialize, Serialize)]
struct StateEntry {
    track_id: String,
    label: String,
    status: StateStatus,
    path: Option<PathBuf>,
    error: Option<String>,
}

impl PlaylistStateStore {
    pub async fn open(
        directory: &Path,
        owner: &str,
        kind: &str,
        jobs: &[PlaylistJob],
    ) -> Result<Self> {
        let path = directory.join(STATE_FILE);
        let old = match tokio::fs::read(&path).await {
            Ok(bytes) => serde_json::from_slice::<PlaylistState>(&bytes).ok(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        let old_entries = old
            .filter(|state| {
                state.version == STATE_VERSION && state.owner == owner && state.kind == kind
            })
            .map(|state| state.entries)
            .unwrap_or_default();
        let entries = jobs
            .iter()
            .map(|job| {
                let old = old_entries.get(&job.index);
                (
                    job.index,
                    StateEntry {
                        track_id: job.track_id.clone(),
                        label: job.label.clone(),
                        status: StateStatus::Pending,
                        path: old.and_then(|entry| entry.path.clone()),
                        error: None,
                    },
                )
            })
            .collect();
        let store = Self {
            path,
            state: Arc::new(Mutex::new(PlaylistState {
                version: STATE_VERSION,
                owner: owner.to_owned(),
                kind: kind.to_owned(),
                updated_at: unix_timestamp()?,
                entries,
            })),
            write_lock: Arc::new(Mutex::new(())),
        };
        store.save().await?;
        Ok(store)
    }

    pub async fn record(
        &self,
        index: usize,
        status: StateStatus,
        path: Option<&Path>,
        error: Option<&str>,
    ) -> Result<()> {
        {
            let mut state = self.state.lock().await;
            state.updated_at = unix_timestamp()?;
            let entry = state
                .entries
                .get_mut(&index)
                .context("state entry is missing")?;
            entry.status = status;
            entry.path = path.map(Path::to_owned);
            entry.error = error.map(str::to_owned);
        }
        self.save().await
    }

    async fn save(&self) -> Result<()> {
        let _write_guard = self.write_lock.lock().await;
        let bytes = {
            let state = self.state.lock().await;
            serde_json::to_vec_pretty(&*state)?
        };
        let temporary = self
            .path
            .with_file_name(format!(".{STATE_FILE}.tmp-{}", std::process::id()));
        let mut file = tokio::fs::File::create(&temporary).await?;
        file.write_all(&bytes).await?;
        file.write_all(b"\n").await?;
        file.flush().await?;
        file.sync_all().await?;
        drop(file);
        #[cfg(windows)]
        if tokio::fs::try_exists(&self.path).await? {
            tokio::fs::remove_file(&self.path).await?;
        }
        tokio::fs::rename(&temporary, &self.path).await?;
        Ok(())
    }
}

fn unix_timestamp() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before the Unix epoch")?
        .as_secs())
}
