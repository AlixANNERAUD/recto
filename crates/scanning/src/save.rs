use std::{
    fs,
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

    let tmp = dir.join(format!(".{file_name}.tmp"));
    fs::write(&tmp, bytes).map_err(storage)?;
    let mut n = 0u32;
    loop {
        let candidate = match (n, ext) {
            (0, _) => file_name.to_owned(),
            (n, Some(ext)) => format!("{stem}-{n}.{ext}"),
            (n, None) => format!("{stem}-{n}"),
        };
        let target = dir.join(candidate);
        // Single user, one save at a time: the check-then-rename gap is acceptable.
        if !target.exists() {
            return match fs::rename(&tmp, &target) {
                Ok(()) => Ok(target),
                Err(e) => {
                    let _ = fs::remove_file(&tmp);
                    Err(storage(e))
                }
            };
        }
        n += 1;
    }
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
        assert_eq!(a, dir.path().join("scan.pdf"));
        assert_eq!(b, dir.path().join("scan-1.pdf"));
        assert_eq!(fs::read(&a).unwrap(), b"one");
        assert_eq!(fs::read(&b).unwrap(), b"two");
        let mut names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, ["scan-1.pdf", "scan.pdf"]);
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
