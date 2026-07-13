use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::{io::AsyncWriteExt, sync::Mutex};

use crate::DownloadJob;

const STATE_VERSION: u32 = 3;
const STATE_FILE: &str = ".ym-download-state.json";
const SAVE_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Clone)]
pub struct CollectionStateStore {
    directory: PathBuf,
    path: PathBuf,
    state: Arc<Mutex<CollectionState>>,
    write_lock: Arc<Mutex<()>>,
    last_saved: Arc<Mutex<Instant>>,
}

#[derive(Debug, Default)]
pub struct CollectionPlan {
    pub known_paths: usize,
    pub stale_paths: Vec<PathBuf>,
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
struct CollectionState {
    version: u32,
    source_kind: String,
    source_id: String,
    updated_at: u64,
    entries: BTreeMap<usize, StateEntry>,
}

#[derive(Debug, Deserialize, Serialize)]
struct StateEntry {
    track_id: String,
    label: String,
    desired_stem: PathBuf,
    status: StateStatus,
    path: Option<PathBuf>,
    error: Option<String>,
}

impl CollectionStateStore {
    pub async fn plan(
        directory: &Path,
        source_kind: &str,
        source_id: &str,
        jobs: &[DownloadJob],
    ) -> Result<CollectionPlan> {
        let old_entries = load_entries(directory, source_kind, source_id).await?;
        let desired = jobs
            .iter()
            .map(|job| (job.track_id.as_str(), desired_stem(directory, job)))
            .collect::<Vec<_>>();
        let known_paths = old_entries
            .values()
            .filter(|entry| {
                desired.iter().any(|(track_id, stem)| {
                    entry.track_id == *track_id && entry.desired_stem == *stem
                }) && entry.path.is_some()
            })
            .count();
        let stale_paths = old_entries
            .values()
            .filter(|entry| {
                !desired.iter().any(|(track_id, stem)| {
                    entry.track_id == *track_id && entry.desired_stem == *stem
                })
            })
            .filter_map(|entry| entry.path.clone())
            .collect();
        Ok(CollectionPlan {
            known_paths,
            stale_paths,
        })
    }

    pub async fn open(
        directory: &Path,
        source_kind: &str,
        source_id: &str,
        jobs: &[DownloadJob],
    ) -> Result<Self> {
        let path = directory.join(STATE_FILE);
        let old_entries = load_entries(directory, source_kind, source_id).await?;
        let entries = jobs
            .iter()
            .map(|job| {
                let stem = desired_stem(directory, job);
                let old = old_entries
                    .values()
                    .find(|entry| entry.track_id == job.track_id && entry.desired_stem == stem);
                (
                    job.index,
                    StateEntry {
                        track_id: job.track_id.clone(),
                        label: job.label.clone(),
                        desired_stem: stem,
                        status: StateStatus::Pending,
                        path: old.and_then(|entry| entry.path.clone()),
                        error: None,
                    },
                )
            })
            .collect();
        let store = Self {
            directory: directory.to_owned(),
            path,
            state: Arc::new(Mutex::new(CollectionState {
                version: STATE_VERSION,
                source_kind: source_kind.to_owned(),
                source_id: source_id.to_owned(),
                updated_at: unix_timestamp()?,
                entries,
            })),
            write_lock: Arc::new(Mutex::new(())),
            last_saved: Arc::new(Mutex::new(Instant::now())),
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
            entry.path = path.map(|path| {
                path.strip_prefix(&self.directory)
                    .unwrap_or(path)
                    .to_owned()
            });
            entry.error = error.map(str::to_owned);
        }
        self.save_if_due().await
    }

    pub async fn flush(&self) -> Result<()> {
        self.save().await?;
        *self.last_saved.lock().await = Instant::now();
        Ok(())
    }

    async fn save_if_due(&self) -> Result<()> {
        let mut last_saved = self.last_saved.lock().await;
        if last_saved.elapsed() < SAVE_INTERVAL {
            return Ok(());
        }
        self.save().await?;
        *last_saved = Instant::now();
        Ok(())
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

async fn load_entries(
    directory: &Path,
    source_kind: &str,
    source_id: &str,
) -> Result<BTreeMap<usize, StateEntry>> {
    let path = directory.join(STATE_FILE);
    let old = match tokio::fs::read(path).await {
        Ok(bytes) => serde_json::from_slice::<CollectionState>(&bytes).ok(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    Ok(old
        .filter(|state| {
            state.version == STATE_VERSION
                && state.source_kind == source_kind
                && state.source_id == source_id
        })
        .map(|state| state.entries)
        .unwrap_or_default())
}

fn desired_stem(directory: &Path, job: &DownloadJob) -> PathBuf {
    job.directory
        .strip_prefix(directory)
        .unwrap_or_else(|_| Path::new(""))
        .join(&job.stem)
}

fn unix_timestamp() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before the Unix epoch")?
        .as_secs())
}
