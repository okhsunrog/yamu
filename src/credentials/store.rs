use std::{
    fs::{self, File, OpenOptions},
    io::{self, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use directories::ProjectDirs;

use super::{Credentials, Error, Result};

pub const DEFAULT_PROFILE: &str = "default";
pub const TOKEN_ENV: &str = "YANDEX_MUSIC_TOKEN";

/// Versioned file-backed credential storage shared by local applications.
#[derive(Clone, Debug)]
pub struct CredentialStore {
    root: PathBuf,
}

/// An exclusive advisory lock for one credential profile.
pub struct ProfileLock {
    file: File,
    path: PathBuf,
}

impl std::fmt::Debug for ProfileLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProfileLock")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl ProfileLock {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ProfileLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

impl CredentialStore {
    pub fn open_default() -> Result<Self> {
        let dirs = ProjectDirs::from_path(PathBuf::from("yandex-music-rs"))
            .ok_or(Error::StateDirectoryUnavailable)?;
        let root = dirs
            .state_dir()
            .unwrap_or_else(|| dirs.data_local_dir())
            .to_owned();
        Ok(Self::at(root))
    }

    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn profile_path(&self, profile: &str) -> Result<PathBuf> {
        validate_profile(profile)?;
        Ok(self.root.join("profiles").join(format!("{profile}.json")))
    }

    pub fn lock_path(&self, profile: &str) -> Result<PathBuf> {
        validate_profile(profile)?;
        Ok(self.root.join("profiles").join(format!("{profile}.lock")))
    }

    /// Acquires an exclusive cross-process advisory lock for a profile.
    pub fn lock_profile(&self, profile: &str) -> Result<ProfileLock> {
        let path = self.lock_path(profile)?;
        let directory = path
            .parent()
            .expect("a profile lock path always has a parent directory");
        self.prepare_directory(directory)?;

        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options
            .open(&path)
            .map_err(|source| io_error(&path, source))?;
        secure_file(&path, &file)?;
        file.lock().map_err(|source| io_error(&path, source))?;
        Ok(ProfileLock { file, path })
    }

    /// Loads an environment override, falling back to the persisted profile.
    pub fn load_effective(&self, profile: &str) -> Result<Credentials> {
        match Self::load_environment()? {
            Some(credentials) => Ok(credentials),
            None => self.load(profile),
        }
    }

    pub(crate) fn load_environment() -> Result<Option<Credentials>> {
        std::env::var(TOKEN_ENV)
            .ok()
            .filter(|token| !token.is_empty())
            .map(Credentials::from_access_token)
            .transpose()
    }

    pub fn load(&self, profile: &str) -> Result<Credentials> {
        let path = self.profile_path(profile)?;
        let file = match File::open(&path) {
            Ok(file) => file,
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                return Err(Error::ProfileNotFound {
                    profile: profile.to_owned(),
                    path,
                });
            }
            Err(source) => return Err(io_error(path, source)),
        };

        check_file_permissions(&path, &file)?;
        let credentials: Credentials =
            serde_json::from_reader(BufReader::new(file)).map_err(|source| Error::Json {
                path: path.clone(),
                source,
            })?;
        credentials.validate_version()?;
        Ok(credentials)
    }

    pub fn save(&self, profile: &str, credentials: &Credentials) -> Result<PathBuf> {
        let _lock = self.lock_profile(profile)?;
        self.save_unlocked(profile, credentials)
    }

    pub(crate) fn save_unlocked(
        &self,
        profile: &str,
        credentials: &Credentials,
    ) -> Result<PathBuf> {
        credentials.validate_version()?;
        let path = self.profile_path(profile)?;
        let directory = path
            .parent()
            .expect("a profile path always has a parent directory");
        self.prepare_directory(directory)?;

        let temporary = temporary_path(directory, profile)?;
        let result = write_credentials(&temporary, credentials)
            .and_then(|()| replace_file(&temporary, &path));
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result?;

        sync_directory(directory)?;
        Ok(path)
    }

    pub fn delete(&self, profile: &str) -> Result<bool> {
        let _lock = self.lock_profile(profile)?;
        let path = self.profile_path(profile)?;
        match fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(source) => Err(io_error(path, source)),
        }
    }

    pub fn exists(&self, profile: &str) -> Result<bool> {
        Ok(self.profile_path(profile)?.is_file())
    }

    fn prepare_directory(&self, directory: &Path) -> Result<()> {
        fs::create_dir_all(directory).map_err(|source| io_error(directory, source))?;
        secure_directory(&self.root)?;
        secure_directory(directory)
    }
}

fn validate_profile(profile: &str) -> Result<()> {
    let valid = !profile.is_empty()
        && profile.len() <= 64
        && profile
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'));
    if valid {
        Ok(())
    } else {
        Err(Error::InvalidProfile(profile.to_owned()))
    }
}

fn temporary_path(directory: &Path, profile: &str) -> Result<PathBuf> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| Error::InvalidSystemClock)?
        .as_nanos();
    Ok(directory.join(format!(
        ".{profile}.json.tmp-{}-{nonce}",
        std::process::id()
    )))
}

fn write_credentials(path: &Path, credentials: &Credentials) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    let file = options
        .open(path)
        .map_err(|source| io_error(path, source))?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer_pretty(&mut writer, credentials).map_err(|source| Error::Json {
        path: path.to_owned(),
        source,
    })?;
    writer
        .write_all(b"\n")
        .map_err(|source| io_error(path, source))?;
    writer.flush().map_err(|source| io_error(path, source))?;
    writer
        .get_ref()
        .sync_all()
        .map_err(|source| io_error(path, source))
}

fn replace_file(temporary: &Path, destination: &Path) -> Result<()> {
    #[cfg(windows)]
    if destination.exists() {
        fs::remove_file(destination).map_err(|source| io_error(destination, source))?;
    }
    fs::rename(temporary, destination).map_err(|source| io_error(destination, source))
}

#[cfg(unix)]
fn secure_directory(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|source| io_error(path, source))
}

#[cfg(not(unix))]
fn secure_directory(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn check_file_permissions(path: &Path, file: &File) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mode = file
        .metadata()
        .map_err(|source| io_error(path, source))?
        .permissions()
        .mode()
        & 0o777;
    if mode & !0o600 == 0 && mode & 0o400 != 0 {
        Ok(())
    } else {
        Err(Error::InsecurePermissions {
            path: path.to_owned(),
            mode,
        })
    }
}

#[cfg(unix)]
fn secure_file(path: &Path, file: &File) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    file.set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(|source| io_error(path, source))
}

#[cfg(not(unix))]
fn secure_file(_path: &Path, _file: &File) -> Result<()> {
    Ok(())
}

#[cfg(not(unix))]
fn check_file_permissions(_path: &Path, _file: &File) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|source| io_error(path, source))
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<()> {
    Ok(())
}

fn io_error(path: impl AsRef<Path>, source: io::Error) -> Error {
    Error::Io {
        path: path.as_ref().to_owned(),
        source,
    }
}
