use std::{
    collections::HashMap,
    fs,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::PathBuf,
    sync::Mutex,
    time::{Duration, SystemTime},
};

use chrono::{DateTime, Local};
use uuid::Uuid;

use crate::{error::ScanError, model::OutputFormat};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanResult {
    pub id: Uuid,
    pub owner: String,
    pub format: OutputFormat,
    pub pages: usize,
    pub created: SystemTime,
    /// `scan-YYYYMMDD-HHMMSS.ext` in local time, the name a download or save suggests.
    pub file_name: String,
}

/// Holds finished scans on disk until they expire. The I/O is blocking: callers run it through
/// `spawn_blocking`.
pub struct ResultStore {
    dir: PathBuf,
    ttl: Duration,
    results: Mutex<HashMap<Uuid, ScanResult>>,
    /// Removes the directory of [`private`](Self::private) with the store.
    _owned: Option<tempfile::TempDir>,
}

impl ResultStore {
    /// A store in `dir`, which must exist: the store never creates it, so a missing one fails
    /// every scan rather than being made with whatever rights the process happens to have.
    pub fn new(dir: PathBuf, ttl: Duration) -> Self {
        Self {
            dir,
            ttl,
            results: Mutex::new(HashMap::new()),
            _owned: None,
        }
    }

    /// A store in a new directory of the system's temporary one, named at random and readable by
    /// recto alone, so no other local user can have made it first or read what it holds. It is
    /// removed with the store. Results of an earlier run were only indexed in memory, so starting
    /// empty loses nothing.
    pub fn private(ttl: Duration) -> std::io::Result<Self> {
        let owned = tempfile::Builder::new()
            .prefix("recto-scans-")
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()?;
        let mut store = Self::new(owned.path().to_owned(), ttl);
        store._owned = Some(owned);
        Ok(store)
    }

    /// A store whose every scan fails with a storage error, for when [`private`](Self::private)
    /// could not make its directory: its path is random and never created.
    pub fn unusable(ttl: Duration) -> Self {
        Self::new(
            std::env::temp_dir().join(format!("recto-scans-{}", Uuid::new_v4())),
            ttl,
        )
    }

    fn path(&self, result: &ScanResult) -> PathBuf {
        // Named by id, never by the display name, so two scans in one second cannot collide.
        self.dir
            .join(format!("{}.{}", result.id, result.format.extension()))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<Uuid, ScanResult>> {
        self.results
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn put(
        &self,
        owner: &str,
        format: OutputFormat,
        pages: usize,
        bytes: &[u8],
    ) -> Result<ScanResult, ScanError> {
        let created = SystemTime::now();
        let local: DateTime<Local> = created.into();
        let result = ScanResult {
            id: Uuid::new_v4(),
            owner: owner.to_owned(),
            format,
            pages,
            created,
            file_name: format!(
                "{}.{}",
                local.format("scan-%Y%m%d-%H%M%S"),
                format.extension()
            ),
        };
        let storage = |e: std::io::Error| ScanError::Storage {
            path: self.dir.clone(),
            reason: e.to_string(),
        };
        // Readable by recto alone, like its directory; never over an existing file.
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(self.path(&result))
            .and_then(|mut file| file.write_all(bytes))
            .map_err(storage)?;
        self.lock().insert(result.id, result.clone());
        Ok(result)
    }

    /// A result of another owner looks exactly like a missing one.
    pub fn get(&self, owner: &str, id: Uuid) -> Option<ScanResult> {
        self.lock().get(&id).filter(|r| r.owner == owner).cloned()
    }

    pub fn read(&self, result: &ScanResult) -> Result<Vec<u8>, ScanError> {
        let path = self.path(result);
        fs::read(&path).map_err(|e| ScanError::Storage {
            path,
            reason: e.to_string(),
        })
    }

    /// Whether `result` has outlived the store's time to live at `now`. [`get`](Self::get) still
    /// returns it until [`purge_expired`](Self::purge_expired) runs, so readers check this too.
    pub fn is_expired(&self, result: &ScanResult, now: SystemTime) -> bool {
        result
            .created
            .checked_add(self.ttl)
            .is_none_or(|end| end <= now)
    }

    pub fn purge_expired(&self, now: SystemTime) {
        let mut results = self.lock();
        results.retain(|_, r| {
            let expired = self.is_expired(r, now);
            if expired {
                // A file that is already gone is the goal state.
                let _ = fs::remove_file(self.path(r));
            }
            !expired
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::temp_dir;

    #[test]
    fn store_round_trip_and_owner_isolation() {
        let dir = temp_dir();
        let store = ResultStore::new(dir.path().to_owned(), Duration::from_secs(60));
        let r = store.put("alice", OutputFormat::Pdf, 2, b"%PDF").unwrap();
        let mode = fs::metadata(store.path(&r)).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(r.pages, 2);
        assert!(r.file_name.starts_with("scan-") && r.file_name.ends_with(".pdf"));
        assert_eq!(r.file_name.len(), "scan-20260101-120000.pdf".len());
        let got = store.get("alice", r.id).unwrap();
        assert_eq!(store.read(&got).unwrap(), b"%PDF");
        assert_eq!(store.get("bob", r.id), None);
        assert_eq!(store.get("alice", Uuid::new_v4()), None);
    }

    #[test]
    fn a_private_store_is_new_closed_and_removed_with_it() {
        let store = ResultStore::private(Duration::from_secs(60)).unwrap();
        let dir = store.dir.clone();
        assert!(
            dir.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("recto-scans-")
        );
        let mode = fs::metadata(&dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
        store.put("alice", OutputFormat::Png, 1, b"png").unwrap();
        drop(store);
        assert!(!dir.exists());
    }

    #[test]
    fn an_unusable_store_fails_without_creating_its_directory() {
        let store = ResultStore::unusable(Duration::from_secs(60));
        assert!(matches!(
            store.put("alice", OutputFormat::Png, 1, b"png"),
            Err(ScanError::Storage { .. })
        ));
        assert!(!store.dir.exists());
    }

    #[test]
    fn store_purges_expired() {
        let dir = temp_dir();
        let store = ResultStore::new(dir.path().to_owned(), Duration::from_secs(60));
        let r = store.put("alice", OutputFormat::Png, 1, b"png").unwrap();
        assert!(!store.is_expired(&r, r.created + Duration::from_secs(59)));
        assert!(store.is_expired(&r, r.created + Duration::from_secs(60)));
        store.purge_expired(r.created + Duration::from_secs(59));
        assert!(store.get("alice", r.id).is_some());
        store.purge_expired(r.created + Duration::from_secs(61));
        assert!(store.get("alice", r.id).is_none());
        assert!(!store.path(&r).exists());
    }
}
