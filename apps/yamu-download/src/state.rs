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
    #[cfg(test)]
    pub async fn plan(
        directory: &Path,
        source_kind: &str,
        source_id: &str,
        jobs: &[DownloadJob],
    ) -> Result<CollectionPlan> {
        Self::plan_with_aliases(directory, source_kind, source_id, &[], jobs).await
    }

    pub async fn plan_with_aliases(
        directory: &Path,
        source_kind: &str,
        source_id: &str,
        source_aliases: &[String],
        jobs: &[DownloadJob],
    ) -> Result<CollectionPlan> {
        let old_state =
            load_state_with_aliases(directory, source_kind, source_id, source_aliases).await?;
        let desired = jobs
            .iter()
            .map(|job| (job.track_id.as_str(), desired_stem(directory, job)))
            .collect::<Vec<_>>();
        let old_entries = old_state
            .iter()
            .flat_map(|state| state.entries.values())
            .collect::<Vec<_>>();
        let known_paths = old_entries
            .iter()
            .filter(|entry| {
                desired.iter().any(|(track_id, stem)| {
                    entry.track_id == *track_id && entry.desired_stem == *stem
                }) && entry.path.is_some()
            })
            .count();
        let active_paths = old_entries
            .iter()
            .filter(|entry| {
                desired.iter().any(|(track_id, stem)| {
                    entry.track_id == *track_id && entry.desired_stem == *stem
                })
            })
            .filter_map(|entry| entry.path.as_ref())
            .collect::<Vec<_>>();
        let mut stale_paths = Vec::new();
        let stale_entries = old_state
            .iter()
            .flat_map(|state| state.stale_entries.iter())
            .chain(old_entries.into_iter().filter(|entry| {
                !desired.iter().any(|(track_id, stem)| {
                    entry.track_id == *track_id && entry.desired_stem == *stem
                })
            }));
        for path in stale_entries.filter_map(|entry| entry.path.clone()) {
            if !active_paths.contains(&&path) && !stale_paths.contains(&path) {
                stale_paths.push(path);
            }
        }
        Ok(CollectionPlan {
            known_paths,
            stale_paths,
        })
    }

    #[cfg(test)]
    pub async fn open(
        directory: &Path,
        source_kind: &str,
        source_id: &str,
        jobs: &[DownloadJob],
    ) -> Result<Self> {
        Self::open_with_aliases(directory, source_kind, source_id, &[], jobs).await
    }

    pub async fn open_with_aliases(
        directory: &Path,
        source_kind: &str,
        source_id: &str,
        source_aliases: &[String],
        jobs: &[DownloadJob],
    ) -> Result<Self> {
        let path = directory.join(STATE_FILE);
        let old_state =
            load_state_with_aliases(directory, source_kind, source_id, source_aliases).await?;
        let old_entries = old_state
            .iter()
            .flat_map(|state| state.entries.values())
            .collect::<Vec<_>>();
        let desired = jobs
            .iter()
            .map(|job| (job.track_id.as_str(), desired_stem(directory, job)))
            .collect::<Vec<_>>();
        let entries: BTreeMap<usize, StateEntry> = jobs
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
        let active_paths = entries
            .values()
            .filter_map(|entry| entry.path.as_ref())
            .collect::<Vec<_>>();
        let mut stale_entries = Vec::new();
        let stale_candidates = old_state
            .iter()
            .flat_map(|state| state.stale_entries.iter())
            .chain(old_entries.into_iter().filter(|entry| {
                !desired.iter().any(|(track_id, stem)| {
                    entry.track_id == *track_id && entry.desired_stem == *stem
                })
            }));
        for entry in stale_candidates.filter(|entry| entry.path.is_some()) {
            if active_paths.contains(&entry.path.as_ref().expect("path was checked"))
                || stale_entries
                    .iter()
                    .any(|stale: &StateEntry| stale.path == entry.path)
            {
                continue;
            }
            stale_entries.push(entry.clone());
        }
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
        let relative_path = path.map(|path| {
            path.strip_prefix(&self.directory)
                .unwrap_or(path)
                .to_owned()
        });
        {
            let mut state = self.state.lock().await;
            state.updated_at = unix_timestamp()?;
            let stale_entry = {
                let entry = state
                    .entries
                    .get_mut(&index)
                    .context("state entry is missing")?;
                let previous = entry.clone();
                entry.status = status;
                if let Some(path) = &relative_path {
                    entry.path = Some(path.clone());
                }
                entry.error = error.map(str::to_owned);
                match (previous.path.as_ref(), relative_path.as_ref()) {
                    (Some(previous_path), Some(path)) if previous_path != path => Some(previous),
                    _ => None,
                }
            };
            if let Some(path) = &relative_path {
                state
                    .stale_entries
                    .retain(|entry| entry.path.as_ref() != Some(path));
            }
            if let Some(stale_entry) = stale_entry
                && !state
                    .stale_entries
                    .iter()
                    .any(|entry| entry.path == stale_entry.path)
            {
                state.stale_entries.push(stale_entry);
            }
        }
        self.save_if_due().await
    }

    pub async fn stale_paths(&self) -> Vec<PathBuf> {
        let state = self.state.lock().await;
        let mut paths = Vec::new();
        for path in state
            .stale_entries
            .iter()
            .filter_map(|entry| entry.path.clone())
        {
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
        paths
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
            crate::atomic_file::persist(&temporary, &self.path, true)?;
            Ok::<_, anyhow::Error>(())
        }
        .await;
        if result.is_err() {
            let _ = tokio::fs::remove_file(&temporary).await;
        }
        result
    }
}

#[cfg(test)]
async fn load_state(
    directory: &Path,
    source_kind: &str,
    source_id: &str,
) -> Result<Option<CollectionState>> {
    load_state_with_aliases(directory, source_kind, source_id, &[]).await
}

async fn load_state_with_aliases(
    directory: &Path,
    source_kind: &str,
    source_id: &str,
    source_aliases: &[String],
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
            && (state.source_id == source_id
                || source_aliases.iter().any(|alias| alias == &state.source_id))
    }))
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

    fn test_job(directory: &Path) -> DownloadJob {
        DownloadJob {
            index: 1,
            total: 1,
            track_id: "42".to_owned(),
            label: "Artist - Track".to_owned(),
            stem: "01 - Artist - Track".to_owned(),
            directory: directory.to_owned(),
            metadata: crate::metadata::TrackMetadata {
                title: "Track".to_owned(),
                artist: "Artist".to_owned(),
                album: None,
                album_artist: None,
                genre: None,
                year: None,
                track_number: None,
                disc_number: None,
                cover_url: None,
                lyrics: None,
            },
        }
    }

    fn test_directory(label: &str) -> PathBuf {
        let nonce = TEMPORARY_COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "yamu-{label}-state-test-{}-{nonce}",
            std::process::id()
        ))
    }

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

    #[tokio::test]
    async fn keeps_the_previous_extension_as_stale_after_replacement() -> Result<()> {
        let directory = test_directory("extension-change");
        tokio::fs::create_dir_all(&directory).await?;
        let jobs = [test_job(&directory)];
        let store = CollectionStateStore::open(&directory, "liked", "7", &jobs).await?;
        let old_path = directory.join("01 - Artist - Track.flac");
        store
            .record(1, StateStatus::Downloaded, Some(&old_path), None)
            .await?;
        store.flush().await?;

        let store = CollectionStateStore::open(&directory, "liked", "7", &jobs).await?;
        let new_path = directory.join("01 - Artist - Track.mp3");
        store
            .record(1, StateStatus::Downloaded, Some(&new_path), None)
            .await?;
        store.flush().await?;

        assert_eq!(
            store.stale_paths().await,
            [PathBuf::from("01 - Artist - Track.flac")]
        );
        let plan = CollectionStateStore::plan(&directory, "liked", "7", &jobs).await?;
        assert_eq!(plan.known_paths, 1);
        assert_eq!(plan.stale_paths, store.stale_paths().await);
        tokio::fs::remove_dir_all(directory).await?;
        Ok(())
    }

    #[tokio::test]
    async fn a_failed_attempt_preserves_the_last_successful_path() -> Result<()> {
        let directory = test_directory("failed-attempt");
        tokio::fs::create_dir_all(&directory).await?;
        let jobs = [test_job(&directory)];
        let store = CollectionStateStore::open(&directory, "liked", "7", &jobs).await?;
        let audio = directory.join("01 - Artist - Track.flac");
        store
            .record(1, StateStatus::Downloaded, Some(&audio), None)
            .await?;
        store
            .record(1, StateStatus::Failed, None, Some("network error"))
            .await?;
        store.flush().await?;

        let state = load_state(&directory, "liked", "7")
            .await?
            .context("state was not saved")?;
        assert_eq!(
            state.entries[&1].path.as_deref(),
            Some(Path::new("01 - Artist - Track.flac"))
        );
        let plan = CollectionStateStore::plan(&directory, "liked", "7", &jobs).await?;
        assert_eq!(plan.known_paths, 1);
        tokio::fs::remove_dir_all(directory).await?;
        Ok(())
    }

    #[tokio::test]
    async fn accepts_an_owner_kind_manifest_when_syncing_by_uuid() -> Result<()> {
        let directory = test_directory("playlist-source-alias");
        tokio::fs::create_dir_all(&directory).await?;
        let jobs = [test_job(&directory)];
        let owner_kind = "owner:42";
        let old_store =
            CollectionStateStore::open(&directory, "playlist", owner_kind, &jobs).await?;
        let audio = directory.join("01 - Artist - Track.flac");
        old_store
            .record(1, StateStatus::Downloaded, Some(&audio), None)
            .await?;
        old_store.flush().await?;

        let aliases = [owner_kind.to_owned()];
        let uuid = "uuid:fa1b8d08-71c7-3ed8-9c58-8eebbdccdf7f";
        let plan =
            CollectionStateStore::plan_with_aliases(&directory, "playlist", uuid, &aliases, &jobs)
                .await?;
        let migrated =
            CollectionStateStore::open_with_aliases(&directory, "playlist", uuid, &aliases, &jobs)
                .await?;
        migrated.flush().await?;

        assert_eq!(plan.known_paths, 1);
        let state = load_state(&directory, "playlist", uuid)
            .await?
            .context("state was not migrated to the UUID identity")?;
        assert_eq!(
            state.entries[&1].path.as_deref(),
            Some(Path::new("01 - Artist - Track.flac"))
        );
        tokio::fs::remove_dir_all(directory).await?;
        Ok(())
    }
}
