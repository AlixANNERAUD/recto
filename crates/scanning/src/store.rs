use std::{
    collections::HashMap,
    fs,
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
}

impl ResultStore {
    pub fn new(dir: PathBuf, ttl: Duration) -> Self {
        Self {
            dir,
            ttl,
            results: Mutex::new(HashMap::new()),
        }
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
        fs::create_dir_all(&self.dir).map_err(storage)?;
        fs::write(self.path(&result), bytes).map_err(storage)?;
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

    pub fn purge_expired(&self, now: SystemTime) {
        let mut results = self.lock();
        results.retain(|_, r| {
            let expired = r.created.checked_add(self.ttl).is_none_or(|end| end <= now);
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
        let store = ResultStore::new(dir.path().join("results"), Duration::from_secs(60));
        let r = store.put("alice", OutputFormat::Pdf, 2, b"%PDF").unwrap();
        assert_eq!(r.pages, 2);
        assert!(r.file_name.starts_with("scan-") && r.file_name.ends_with(".pdf"));
        assert_eq!(r.file_name.len(), "scan-20260101-120000.pdf".len());
        let got = store.get("alice", r.id).unwrap();
        assert_eq!(store.read(&got).unwrap(), b"%PDF");
        assert_eq!(store.get("bob", r.id), None);
        assert_eq!(store.get("alice", Uuid::new_v4()), None);
    }

    #[test]
    fn store_purges_expired() {
        let dir = temp_dir();
        let store = ResultStore::new(dir.path().to_owned(), Duration::from_secs(60));
        let r = store.put("alice", OutputFormat::Png, 1, b"png").unwrap();
        store.purge_expired(r.created + Duration::from_secs(59));
        assert!(store.get("alice", r.id).is_some());
        store.purge_expired(r.created + Duration::from_secs(61));
        assert!(store.get("alice", r.id).is_none());
        assert!(!store.path(&r).exists());
    }
}
