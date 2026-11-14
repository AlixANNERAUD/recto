use std::{
    fs,
    io::Write as _,
    path::{Path, PathBuf},
};

use crate::error::ScanError;

/// Writes `bytes` into `dir` under `file_name`, or `stem-1.ext`, `stem-2.ext` when taken. The data
/// goes to a hidden temp file first, then is renamed into place, so a folder watcher never sees a
/// half-written scan.
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

    let result = publish(dir, file_name, stem, ext, &tmp).map_err(storage);
    // Gone already when it was renamed into place.
    let _ = fs::remove_file(&tmp);
    result
}

/// Gives the finished temp file its final name, from the best way to the last resort. None of
/// them ever replaces an existing file; a taken name moves on to the next suffix.
fn publish(
    dir: &Path,
    file_name: &str,
    stem: &str,
    ext: Option<&str>,
    tmp: &Path,
) -> std::io::Result<PathBuf> {
    // An atomic rename that refuses to replace: folder watchers see a file moved in
    // (`IN_MOVED_TO`), which every consume-folder watcher handles.
    match place(dir, file_name, stem, ext, |target| {
        rename_no_replace(tmp, target)
    }) {
        Err(e) if rename_unsupported(&e) => {}
        other => return other,
    }
    // Just as atomic, but watchers only see a file created, which older ones that wait for a
    // write to close or a move in can miss.
    match place(dir, file_name, stem, ext, |target| {
        fs::hard_link(tmp, target)
    }) {
        Err(e) if links_unsupported(&e) => {}
        other => return other,
    }
    publish_without_link(dir, file_name, tmp)
}

/// Tries `put` on `file_name`, then `stem-1.ext`, `stem-2.ext`... while the name is taken.
fn place(
    dir: &Path,
    file_name: &str,
    stem: &str,
    ext: Option<&str>,
    mut put: impl FnMut(&Path) -> std::io::Result<()>,
) -> std::io::Result<PathBuf> {
    for n in 0.. {
        let target = dir.join(candidate_name(file_name, stem, ext, n));
        match put(&target) {
            Ok(()) => return Ok(target),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    unreachable!("the suffix counter does not run out")
}

/// `rename(2)` silently replaces an existing file on Unix; `RENAME_NOREPLACE` fails with
/// `EEXIST` instead, atomically.
fn rename_no_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};
    renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE).map_err(std::io::Error::from)
}

/// Filesystems that do not support `RENAME_NOREPLACE` (some SMB, NFS and FUSE mounts) answer
/// `EINVAL`, and kernels before 3.15 `ENOSYS`.
fn rename_unsupported(e: &std::io::Error) -> bool {
    use rustix::io::Errno;
    e.kind() == std::io::ErrorKind::Unsupported
        || [Errno::INVAL, Errno::NOSYS, Errno::OPNOTSUPP]
            .iter()
            .any(|errno| e.raw_os_error() == Some(errno.raw_os_error()))
}

fn candidate_name(file_name: &str, stem: &str, ext: Option<&str>, n: u32) -> String {
    match (n, ext) {
        (0, _) => file_name.to_owned(),
        (n, Some(ext)) => format!("{stem}-{n}.{ext}"),
        (n, None) => format!("{stem}-{n}"),
    }
}

/// SMB and NFS mounts, common for consume folders, often reject hard links.
fn links_unsupported(e: &std::io::Error) -> bool {
    use std::io::ErrorKind::{CrossesDevices, PermissionDenied, TooManyLinks, Unsupported};
    // 95 is ENOTSUP/EOPNOTSUPP on Linux, which `ErrorKind` does not always map.
    matches!(
        e.kind(),
        Unsupported | PermissionDenied | CrossesDevices | TooManyLinks
    ) || e.raw_os_error() == Some(95)
}

/// Last resort when neither renames nor hard links work: copy the finished temp file straight to the target
/// with `create_new`, which still never replaces a file. The trade-off is that the target is
/// briefly partial; consumers such as Paperless act on close-after-write or on a stable file,
/// so that is acceptable where a no-replace atomic publish is impossible.
fn publish_without_link(dir: &Path, file_name: &str, tmp: &Path) -> std::io::Result<PathBuf> {
    let bytes = fs::read(tmp)?;
    let path = Path::new(file_name);
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("scan");
    let ext = path.extension().and_then(|s| s.to_str());
    for n in 0.. {
        let target = dir.join(candidate_name(file_name, stem, ext, n));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
        {
            Ok(mut f) => {
                // The file is ours (create_new), so a failed write may delete it safely.
                if let Err(e) = f.write_all(&bytes).and_then(|()| f.sync_all()) {
                    drop(f);
                    let _ = fs::remove_file(&target);
                    return Err(e);
                }
                return Ok(target);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    unreachable!("the suffix counter does not run out")
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
    fn fallback_suffixes_clashes_and_never_overwrites() {
        let dir = temp_dir();
        let tmp = dir.path().join(".src.tmp");
        fs::write(&tmp, b"new").unwrap();
        fs::write(dir.path().join("scan.pdf"), b"original").unwrap();
        let a = publish_without_link(dir.path(), "scan.pdf", &tmp).unwrap();
        let b = publish_without_link(dir.path(), "scan.pdf", &tmp).unwrap();
        assert_eq!(a, dir.path().join("scan-1.pdf"));
        assert_eq!(b, dir.path().join("scan-2.pdf"));
        assert_eq!(fs::read(dir.path().join("scan.pdf")).unwrap(), b"original");
        assert_eq!(fs::read(&b).unwrap(), b"new");
    }

    #[test]
    fn rename_never_replaces() {
        let dir = temp_dir();
        let new = dir.path().join(".new.tmp");
        let taken = dir.path().join("taken.pdf");
        fs::write(&new, b"new").unwrap();
        fs::write(&taken, b"original").unwrap();
        let refused = rename_no_replace(&new, &taken).unwrap_err();
        assert_eq!(refused.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&taken).unwrap(), b"original");
        let free = dir.path().join("free.pdf");
        rename_no_replace(&new, &free).unwrap();
        assert!(!new.exists());
        assert_eq!(fs::read(&free).unwrap(), b"new");
    }

    #[test]
    fn publishing_moves_the_temp_file_in() {
        // A move, not a second name: watchers see the file arrive whole.
        let dir = temp_dir();
        let tmp = dir.path().join(".scan.tmp");
        fs::write(&tmp, b"new").unwrap();
        fs::write(dir.path().join("scan.pdf"), b"original").unwrap();
        let saved = publish(dir.path(), "scan.pdf", "scan", Some("pdf"), &tmp).unwrap();
        assert_eq!(saved, dir.path().join("scan-1.pdf"));
        assert!(!tmp.exists(), "renamed, not linked");
        assert_eq!(fs::read(&saved).unwrap(), b"new");
        assert_eq!(fs::read(dir.path().join("scan.pdf")).unwrap(), b"original");
    }

    #[test]
    fn unsupported_rename_errors_are_recognised() {
        use std::io::{Error, ErrorKind};
        assert!(rename_unsupported(&Error::from_raw_os_error(
            rustix::io::Errno::INVAL.raw_os_error()
        )));
        assert!(rename_unsupported(&Error::from(ErrorKind::Unsupported)));
        assert!(!rename_unsupported(&Error::from(ErrorKind::AlreadyExists)));
        assert!(!rename_unsupported(&Error::from(
            ErrorKind::PermissionDenied
        )));
    }

    #[test]
    fn unsupported_link_errors_are_recognised() {
        use std::io::{Error, ErrorKind};
        assert!(links_unsupported(&Error::from(ErrorKind::Unsupported)));
        assert!(links_unsupported(&Error::from(ErrorKind::PermissionDenied)));
        assert!(links_unsupported(&Error::from_raw_os_error(95)));
        assert!(!links_unsupported(&Error::from(ErrorKind::NotFound)));
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
