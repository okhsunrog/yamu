//! Reusable audio transfer, normalization, progress, and cancellation pipeline.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use aes::{Aes128, Aes192, Aes256};
use ctr::{
    Ctr128BE,
    cipher::{KeyIvInit as _, StreamCipher as _},
};
use thiserror::Error;
use tokio::{io::AsyncWriteExt as _, sync::watch, time::sleep};

use crate::{
    Client, atomic_file,
    media::{self, MediaBackend},
    models::{AudioCodec, DownloadInfo},
    temporary_file,
};

/// Current stage of a download operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DownloadPhase {
    Connecting,
    Downloading,
    Normalizing,
    Finalizing,
}

/// Observable state changes emitted by [`Downloader::download`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DownloadEvent {
    PhaseChanged(DownloadPhase),
    Progress { downloaded: u64, total: Option<u64> },
    Retrying { source: usize, attempt: u8 },
}

/// Tunable transfer policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DownloadPolicy {
    pub attempts_per_source: u8,
    pub retry_delay: Duration,
    pub progress_interval: Duration,
}

impl Default for DownloadPolicy {
    fn default() -> Self {
        Self {
            attempts_per_source: 3,
            retry_delay: Duration::from_millis(250),
            progress_interval: Duration::from_millis(100),
        }
    }
}

/// Input for one negotiated audio transfer.
#[derive(Clone, Debug)]
pub struct DownloadRequest {
    pub info: DownloadInfo,
    pub destination: PathBuf,
    pub replace: bool,
}

/// Successful transfer details.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DownloadResult {
    pub path: PathBuf,
    pub downloaded: u64,
    pub quality: String,
    pub codec: AudioCodec,
}

/// Cloneable cancellation signal. Cancellation is cooperative and immediate
/// during network transfer and retry delays.
#[derive(Clone, Debug)]
pub struct CancellationToken {
    sender: Arc<watch::Sender<bool>>,
}

impl Default for CancellationToken {
    fn default() -> Self {
        let (sender, _) = watch::channel(false);
        Self {
            sender: Arc::new(sender),
        }
    }
}

impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.sender.send_replace(true);
    }

    pub fn is_cancelled(&self) -> bool {
        *self.sender.borrow()
    }

    /// Waits until cancellation is requested.
    pub async fn cancelled(&self) {
        let mut receiver = self.sender.subscribe();
        while !*receiver.borrow_and_update() && receiver.changed().await.is_ok() {}
    }
}

/// Errors from the reusable download pipeline.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    #[error("download was cancelled")]
    Cancelled,
    #[error("destination already exists: {0}")]
    DestinationExists(PathBuf),
    #[error("the server returned no download URLs")]
    NoSources,
    #[error("all {sources} download sources failed")]
    AllSourcesFailed { sources: usize },
    #[error("invalid AES-CTR decryption key")]
    InvalidDecryptionKey,
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("API request failed: {0}")]
    Api(#[from] crate::Error),
    #[error("media processing failed: {0}")]
    Media(#[from] media::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Audio downloader sharing one transfer implementation across frontends.
#[derive(Clone)]
pub struct Downloader<B> {
    client: Client,
    backend: B,
    policy: DownloadPolicy,
}

impl<B: MediaBackend> Downloader<B> {
    pub fn new(client: Client, backend: B) -> Self {
        Self {
            client,
            backend,
            policy: DownloadPolicy::default(),
        }
    }

    pub fn with_policy(mut self, policy: DownloadPolicy) -> Self {
        self.policy = policy;
        self
    }

    pub async fn download<F>(
        &self,
        request: DownloadRequest,
        cancellation: CancellationToken,
        mut on_event: F,
    ) -> Result<DownloadResult>
    where
        F: FnMut(DownloadEvent),
    {
        if cancellation.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if request.info.urls.is_empty() {
            return Err(Error::NoSources);
        }
        if tokio::fs::try_exists(&request.destination).await? && !request.replace {
            return Err(Error::DestinationExists(request.destination));
        }
        if let Some(parent) = request.destination.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let temporary = temporary_file::sibling(&request.destination, "download", Some("part"));
        let source = temporary_file::sibling(&request.destination, "source", Some("m4a"));
        let normalized = temporary_file::sibling(&request.destination, "normalized", Some("flac"));
        let _ = tokio::fs::remove_file(&temporary).await;
        let _ = tokio::fs::remove_file(&source).await;
        let _ = tokio::fs::remove_file(&normalized).await;

        let transfer_path = if matches!(request.info.codec, AudioCodec::FlacMp4) {
            &source
        } else {
            &temporary
        };
        let downloaded = match self
            .transfer(&request.info, transfer_path, &cancellation, &mut on_event)
            .await
        {
            Ok(downloaded) => downloaded,
            Err(error) => {
                let _ = tokio::fs::remove_file(transfer_path).await;
                return Err(error);
            }
        };

        if cancellation.is_cancelled() {
            let _ = tokio::fs::remove_file(transfer_path).await;
            return Err(Error::Cancelled);
        }

        if matches!(request.info.codec, AudioCodec::FlacMp4) {
            on_event(DownloadEvent::PhaseChanged(DownloadPhase::Normalizing));
            let result = self
                .backend
                .remux_flac(source.clone(), normalized.clone(), true)
                .await;
            let _ = tokio::fs::remove_file(&source).await;
            if let Err(error) = result {
                let _ = tokio::fs::remove_file(&normalized).await;
                return Err(error.into());
            }
            if cancellation.is_cancelled() {
                let _ = tokio::fs::remove_file(&normalized).await;
                return Err(Error::Cancelled);
            }
            on_event(DownloadEvent::PhaseChanged(DownloadPhase::Finalizing));
            if let Err(error) = replace_file(&normalized, &request.destination, request.replace) {
                let _ = tokio::fs::remove_file(&normalized).await;
                return Err(error);
            }
        } else {
            on_event(DownloadEvent::PhaseChanged(DownloadPhase::Finalizing));
            if let Err(error) = replace_file(&temporary, &request.destination, request.replace) {
                let _ = tokio::fs::remove_file(&temporary).await;
                return Err(error);
            }
        }

        Ok(DownloadResult {
            path: request.destination,
            downloaded,
            quality: request.info.quality,
            codec: request.info.codec,
        })
    }

    async fn transfer<F>(
        &self,
        info: &DownloadInfo,
        destination: &Path,
        cancellation: &CancellationToken,
        on_event: &mut F,
    ) -> Result<u64>
    where
        F: FnMut(DownloadEvent),
    {
        let attempts = self.policy.attempts_per_source.max(1);
        for (source_index, url) in info.urls.iter().enumerate() {
            for attempt in 1..=attempts {
                on_event(DownloadEvent::PhaseChanged(DownloadPhase::Connecting));
                match self
                    .transfer_once(info, url, destination, cancellation, on_event)
                    .await
                {
                    Ok(downloaded) => return Ok(downloaded),
                    Err(Error::Cancelled) => return Err(Error::Cancelled),
                    Err(error) if attempt < attempts && is_transient(&error) => {
                        let _ = tokio::fs::remove_file(destination).await;
                        on_event(DownloadEvent::Retrying {
                            source: source_index + 1,
                            attempt: attempt + 1,
                        });
                        let delay = self.policy.retry_delay * u32::from(attempt);
                        tokio::select! {
                            () = cancellation.cancelled() => return Err(Error::Cancelled),
                            () = sleep(delay) => {}
                        }
                    }
                    Err(error) if is_source_failure(&error) => break,
                    Err(error) => return Err(error),
                }
            }
            let _ = tokio::fs::remove_file(destination).await;
        }
        Err(Error::AllSourcesFailed {
            sources: info.urls.len(),
        })
    }

    async fn transfer_once<F>(
        &self,
        info: &DownloadInfo,
        url: &url::Url,
        destination: &Path,
        cancellation: &CancellationToken,
        on_event: &mut F,
    ) -> Result<u64>
    where
        F: FnMut(DownloadEvent),
    {
        let mut response = tokio::select! {
            () = cancellation.cancelled() => return Err(Error::Cancelled),
            response = self.client.open_audio_stream(url) => response?,
        };
        let total = response.content_length();
        on_event(DownloadEvent::PhaseChanged(DownloadPhase::Downloading));
        on_event(DownloadEvent::Progress {
            downloaded: 0,
            total,
        });

        let mut decryptor = info
            .decryption_key
            .as_deref()
            .map(Decryptor::new)
            .transpose()?;
        let mut file = tokio::fs::File::create(destination).await?;
        let mut downloaded = 0_u64;
        let mut last_event = Instant::now()
            .checked_sub(self.policy.progress_interval)
            .unwrap_or_else(Instant::now);

        loop {
            let chunk = tokio::select! {
                () = cancellation.cancelled() => return Err(Error::Cancelled),
                chunk = response.chunk() => chunk.map_err(crate::Error::Http)?,
            };
            let Some(chunk) = chunk else {
                break;
            };
            if let Some(decryptor) = decryptor.as_mut() {
                let mut decrypted = chunk.to_vec();
                decryptor.apply(&mut decrypted);
                file.write_all(&decrypted).await?;
            } else {
                file.write_all(&chunk).await?;
            }
            downloaded += chunk.len() as u64;
            let finished = total.is_some_and(|total| downloaded >= total);
            if finished || last_event.elapsed() >= self.policy.progress_interval {
                on_event(DownloadEvent::Progress { downloaded, total });
                last_event = Instant::now();
            }
        }
        file.flush().await?;
        file.sync_all().await?;
        on_event(DownloadEvent::Progress { downloaded, total });
        Ok(downloaded)
    }
}

enum Decryptor {
    Aes128(Ctr128BE<Aes128>),
    Aes192(Ctr128BE<Aes192>),
    Aes256(Ctr128BE<Aes256>),
}

impl Decryptor {
    fn new(encoded_key: &str) -> Result<Self> {
        let key = hex::decode(encoded_key.trim()).map_err(|_| Error::InvalidDecryptionKey)?;
        let iv = [0_u8; 16];
        match key.len() {
            16 => Ok(Self::Aes128(
                Ctr128BE::<Aes128>::new_from_slices(&key, &iv)
                    .map_err(|_| Error::InvalidDecryptionKey)?,
            )),
            24 => Ok(Self::Aes192(
                Ctr128BE::<Aes192>::new_from_slices(&key, &iv)
                    .map_err(|_| Error::InvalidDecryptionKey)?,
            )),
            32 => Ok(Self::Aes256(
                Ctr128BE::<Aes256>::new_from_slices(&key, &iv)
                    .map_err(|_| Error::InvalidDecryptionKey)?,
            )),
            _ => Err(Error::InvalidDecryptionKey),
        }
    }

    fn apply(&mut self, bytes: &mut [u8]) {
        match self {
            Self::Aes128(cipher) => cipher.apply_keystream(bytes),
            Self::Aes192(cipher) => cipher.apply_keystream(bytes),
            Self::Aes256(cipher) => cipher.apply_keystream(bytes),
        }
    }
}

fn replace_file(source: &Path, destination: &Path, replace: bool) -> Result<()> {
    match atomic_file::persist(source, destination, replace) {
        Err(error) if !replace && error.kind() == std::io::ErrorKind::AlreadyExists => {
            Err(Error::DestinationExists(destination.to_owned()))
        }
        result => result.map_err(Error::from),
    }
}

fn is_transient(error: &Error) -> bool {
    match error {
        Error::Api(crate::Error::Http(error)) => {
            error.is_connect()
                || error.is_timeout()
                || error.status().is_some_and(|status| {
                    status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
                })
        }
        Error::Io(error) => matches!(
            error.kind(),
            std::io::ErrorKind::ConnectionAborted
                | std::io::ErrorKind::ConnectionRefused
                | std::io::ErrorKind::ConnectionReset
                | std::io::ErrorKind::Interrupted
                | std::io::ErrorKind::TimedOut
                | std::io::ErrorKind::UnexpectedEof
        ),
        _ => false,
    }
}

fn is_source_failure(error: &Error) -> bool {
    matches!(error, Error::Api(crate::Error::Http(_)))
}
