use std::{
    fs,
    io::Write as _,
    path::{Path, PathBuf},
};

use crate::error::ScanError;

/// Writes `bytes` into `dir` under `file_name`, or `stem-1.ext`, `stem-2.ext` when taken. The data
/// goes to a hidden temp file first so a folder watcher never sees a half-written scan.
pub fn save_to_folder(dir: &Path, file_name: &str, bytes: &[u8]) -> Result<PathBuf, ScanError> {
    let storage = |e: std::io::Error| ScanError::Storage {
        path: dir.to_owned(),
        reason: e.to_string(),
    };
    // The name may come from the browser; refuse anything that would leave `dir`.
    let name = Path::new(file_name)
        .file_name()
        .filter(|n| *n == std::ffi::OsStr::new(file_name))
        .ok_or_else(|| ScanError::Storage {
            path: dir.to_owned(),
            reason: format!("invalid file name {file_name:?}"),
        })?;
    let path = Path::new(name);
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("scan");
    let ext = path.extension().and_then(|s| s.to_str());

    // Unique and hidden: concurrent saves of one name must not share a temp file, and a watcher
    // that skips dotfiles never sees the partial data. `create_new` refuses a pre-existing file.
    let tmp = dir.join(format!(".{file_name}.{}.tmp", uuid::Uuid::new_v4()));
    let written = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .and_then(|mut f| f.write_all(bytes).and_then(|()| f.sync_all()));
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Err(storage(e));
    }

    let mut n = 0u32;
    let result = loop {
        let candidate = match (n, ext) {
            (0, _) => file_name.to_owned(),
            (n, Some(ext)) => format!("{stem}-{n}.{ext}"),
            (n, None) => format!("{stem}-{n}"),
        };
        let target = dir.join(candidate);
        // A hard link fails atomically with AlreadyExists and never replaces a file, unlike
        // `rename`, which silently overwrites on Unix. Filesystems without hard links are
        // reported as a storage error rather than risking an overwrite.
        match fs::hard_link(&tmp, &target) {
            Ok(()) => break Ok(target),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => n += 1,
            Err(e) => break Err(storage(e)),
        }
    };
    let _ = fs::remove_file(&tmp);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::temp_dir;

    #[test]
    fn save_writes_atomically_and_suffixes_clashes() {
        let dir = temp_dir();
        let a = save_to_folder(dir.path(), "scan.pdf", b"one").unwrap();
        let b = save_to_folder(dir.path(), "scan.pdf", b"two").unwrap();
        let c = save_to_folder(dir.path(), "scan.pdf", b"three").unwrap();
        assert_eq!(c, dir.path().join("scan-2.pdf"));
        assert_eq!(a, dir.path().join("scan.pdf"));
        assert_eq!(b, dir.path().join("scan-1.pdf"));
        assert_eq!(fs::read(&a).unwrap(), b"one");
        assert_eq!(fs::read(&b).unwrap(), b"two");
        let mut names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, ["scan-1.pdf", "scan-2.pdf", "scan.pdf"]);
    }

    #[test]
    fn clashing_save_leaves_existing_file_untouched() {
        let dir = temp_dir();
        fs::write(dir.path().join("scan.pdf"), b"original").unwrap();
        save_to_folder(dir.path(), "scan.pdf", b"new").unwrap();
        assert_eq!(fs::read(dir.path().join("scan.pdf")).unwrap(), b"original");
        assert_eq!(fs::read(dir.path().join("scan-1.pdf")).unwrap(), b"new");
    }

    #[test]
    fn concurrent_saves_never_overwrite() {
        let dir = temp_dir();
        let path = dir.path().to_owned();
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || save_to_folder(&path, "scan.pdf", &[i]).unwrap())
            })
            .collect();
        let mut saved: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        saved.sort();
        saved.dedup();
        assert_eq!(saved.len(), 8);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 8);
    }

    #[test]
    fn save_reports_unwritable_folder() {
        let dir = temp_dir();
        let missing = dir.path().join("missing");
        match save_to_folder(&missing, "scan.pdf", b"x") {
            Err(ScanError::Storage { path, .. }) => assert_eq!(path, missing),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn save_refuses_path_traversal() {
        let dir = temp_dir();
        assert!(matches!(
            save_to_folder(dir.path(), "../evil.pdf", b"x"),
            Err(ScanError::Storage { .. })
        ));
    }
}
