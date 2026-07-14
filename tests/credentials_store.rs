#![cfg(feature = "credentials")]

use std::fs;

use yamu::credentials::{CredentialStore, Credentials, Error};

struct TestDirectory(std::path::PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("yandex-music-credentials-{}", uuid::Uuid::new_v4()));
        Self(path)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn saves_loads_and_deletes_profile() {
    let directory = TestDirectory::new();
    let store = CredentialStore::at(&directory.0);
    let credentials = Credentials::from_access_token("access-secret").unwrap();

    let path = store.save("default", &credentials).unwrap();
    assert!(path.is_file());
    assert!(store.exists("default").unwrap());

    let loaded = store.load("default").unwrap();
    assert_eq!(loaded.access_token(), "access-secret");
    assert!(!format!("{loaded:?}").contains("access-secret"));

    assert!(store.delete("default").unwrap());
    assert!(!store.delete("default").unwrap());
}

#[test]
fn save_new_never_replaces_an_existing_profile() {
    let directory = TestDirectory::new();
    let store = CredentialStore::at(&directory.0);
    let original = Credentials::from_access_token("original-secret").unwrap();
    let contender = Credentials::from_access_token("contender-secret").unwrap();

    store.save_new("default", &original).unwrap();
    assert!(matches!(
        store.save_new("default", &contender),
        Err(Error::ProfileAlreadyExists { .. })
    ));

    assert_eq!(
        store.load("default").unwrap().access_token(),
        "original-secret"
    );
}

#[test]
fn concurrent_save_new_has_exactly_one_winner() {
    use std::sync::{Arc, Barrier};

    let directory = TestDirectory::new();
    let store = CredentialStore::at(&directory.0);
    let barrier = Arc::new(Barrier::new(2));
    let contenders = ["first-secret", "second-secret"]
        .into_iter()
        .map(|token| {
            let store = store.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                let credentials = Credentials::from_access_token(token).unwrap();
                barrier.wait();
                (token, store.save_new("default", &credentials))
            })
        })
        .collect::<Vec<_>>();
    let results = contenders
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect::<Vec<_>>();

    let winner = results
        .iter()
        .find_map(|(token, result)| result.as_ref().ok().map(|_| *token))
        .unwrap();
    assert_eq!(
        results.iter().filter(|(_, result)| result.is_ok()).count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|(_, result)| matches!(result, Err(Error::ProfileAlreadyExists { .. })))
            .count(),
        1
    );
    assert_eq!(store.load("default").unwrap().access_token(), winner);
}

#[test]
fn rejects_path_traversal_profiles() {
    let directory = TestDirectory::new();
    let store = CredentialStore::at(&directory.0);

    assert!(matches!(
        store.profile_path("../secret"),
        Err(Error::InvalidProfile(_))
    ));
}

#[cfg(unix)]
#[test]
fn saved_paths_have_private_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let directory = TestDirectory::new();
    let store = CredentialStore::at(&directory.0);
    let credentials = Credentials::from_access_token("access-secret").unwrap();
    let path = store.save("default", &credentials).unwrap();

    let file_mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    let directory_mode = fs::metadata(path.parent().unwrap())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    let root_mode = fs::metadata(&directory.0).unwrap().permissions().mode() & 0o777;
    let lock_mode = fs::metadata(store.lock_path("default").unwrap())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(file_mode, 0o600);
    assert_eq!(lock_mode, 0o600);
    assert_eq!(directory_mode, 0o700);
    assert_eq!(root_mode, 0o700);
}

#[cfg(unix)]
#[test]
fn loads_read_only_profile() {
    use std::os::unix::fs::PermissionsExt;

    let directory = TestDirectory::new();
    let store = CredentialStore::at(&directory.0);
    let credentials = Credentials::from_access_token("access-secret").unwrap();
    let path = store.save("default", &credentials).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).unwrap();

    assert_eq!(
        store.load("default").unwrap().access_token(),
        "access-secret"
    );
}

#[test]
fn profile_lock_excludes_another_file_handle() {
    let directory = TestDirectory::new();
    let store = CredentialStore::at(&directory.0);
    let guard = store.lock_profile("default").unwrap();
    let contender = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(guard.path())
        .unwrap();

    assert!(matches!(
        contender.try_lock(),
        Err(std::fs::TryLockError::WouldBlock)
    ));
    drop(guard);
    contender.try_lock().unwrap();
    contender.unlock().unwrap();
}
