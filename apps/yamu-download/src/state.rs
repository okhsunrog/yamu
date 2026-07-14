use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::{io::AsyncWriteExt, sync::Mutex};

use crate::DownloadJob;

const STATE_VERSION: u32 = 3;
const STATE_FILE: &str = ".yamu-download-state.json";
const LEGACY_STATE_FILE: &str = ".ym-download-state.json";
const SAVE_INTERVAL: Duration = Duration::from_secs(1);
static TEMPORARY_COUNTER: AtomicU64 = AtomicU64::new(0);

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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    stale_entries: Vec<StateEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
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
        let old_state = load_state(directory, source_kind, source_id).await?;
        let desired = jobs
            .iter()
            .map(|job| (job.track_id.as_str(), desired_stem(directory, job)))
            .collect::<Vec<_>>();
        let old_entries = old_state.iter().flat_map(all_entries);
        let known_paths = old_entries
            .clone()
            .filter(|entry| {
                desired.iter().any(|(track_id, stem)| {
                    entry.track_id == *track_id && entry.desired_stem == *stem
                }) && entry.path.is_some()
            })
            .count();
        let mut stale_paths = Vec::new();
        for path in old_entries
            .filter(|entry| {
                !desired.iter().any(|(track_id, stem)| {
                    entry.track_id == *track_id && entry.desired_stem == *stem
                })
            })
            .filter_map(|entry| entry.path.clone())
        {
            if !stale_paths.contains(&path) {
                stale_paths.push(path);
            }
        }
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
        let old_state = load_state(directory, source_kind, source_id).await?;
        let old_entries = old_state.iter().flat_map(all_entries).collect::<Vec<_>>();
        let desired = jobs
            .iter()
            .map(|job| (job.track_id.as_str(), desired_stem(directory, job)))
            .collect::<Vec<_>>();
        let entries = jobs
            .iter()
            .map(|job| {
                let stem = desired_stem(directory, job);
                let old = old_entries
                    .iter()
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
        let stale_entries = old_entries
            .into_iter()
            .filter(|entry| entry.path.is_some())
            .filter(|entry| {
                !desired.iter().any(|(track_id, stem)| {
                    entry.track_id == *track_id && entry.desired_stem == *stem
                })
            })
            .cloned()
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
                stale_entries,
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

    pub async fn forget_stale_path(&self, path: &Path) -> Result<()> {
        let relative = path.strip_prefix(&self.directory).unwrap_or(path);
        let mut state = self.state.lock().await;
        state.updated_at = unix_timestamp()?;
        state
            .stale_entries
            .retain(|entry| entry.path.as_deref() != Some(relative));
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
        let nonce = TEMPORARY_COUNTER.fetch_add(1, Ordering::Relaxed);
        let temporary = self
            .path
            .with_file_name(format!(".{STATE_FILE}.tmp-{}-{nonce}", std::process::id()));
        let result = async {
            let mut file = tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .await?;
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
            Ok::<_, anyhow::Error>(())
        }
        .await;
        if result.is_err() {
            let _ = tokio::fs::remove_file(&temporary).await;
        }
        result
    }
}

async fn load_state(
    directory: &Path,
    source_kind: &str,
    source_id: &str,
) -> Result<Option<CollectionState>> {
    let bytes = match tokio::fs::read(directory.join(STATE_FILE)).await {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match tokio::fs::read(directory.join(LEGACY_STATE_FILE)).await {
                Ok(bytes) => Some(bytes),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(error.into()),
            }
        }
        Err(error) => return Err(error.into()),
    };
    let old = bytes.and_then(|bytes| serde_json::from_slice::<CollectionState>(&bytes).ok());
    Ok(old.filter(|state| {
        state.version == STATE_VERSION
            && state.source_kind == source_kind
            && state.source_id == source_id
    }))
}

fn all_entries(state: &CollectionState) -> impl Clone + Iterator<Item = &StateEntry> {
    state.entries.values().chain(state.stale_entries.iter())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn loads_manifest_written_by_the_old_binary_name() -> Result<()> {
        let nonce = TEMPORARY_COUNTER.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "yamu-legacy-state-test-{}-{nonce}",
            std::process::id()
        ));
        tokio::fs::create_dir_all(&directory).await?;
        let state = CollectionState {
            version: STATE_VERSION,
            source_kind: "playlist".to_owned(),
            source_id: "owner:42".to_owned(),
            updated_at: 0,
            entries: BTreeMap::new(),
            stale_entries: Vec::new(),
        };
        tokio::fs::write(
            directory.join(LEGACY_STATE_FILE),
            serde_json::to_vec(&state)?,
        )
        .await?;

        let loaded = load_state(&directory, "playlist", "owner:42").await?;
        tokio::fs::remove_dir_all(&directory).await?;

        assert!(loaded.is_some());
        Ok(())
    }
}
