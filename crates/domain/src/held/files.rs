//! Where held documents are kept: one file each under `RECTO_DATA_DIR/held`, readable by recto
//! alone. Every function blocks: call them through [`HeldFiles::blocking`].
//!
//! A document is written to a temporary file then renamed, so a crash never leaves half a
//! document under a name a row could point at. A crash between the rename and the row's insert
//! leaves a file no row names: [`HeldFiles::orphans`] finds those.

use std::{
    collections::HashSet,
    fs::{self, OpenOptions},
    io::{self, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};

use data_encoding::HEXLOWER;

/// The directory under the data directory.
const DIRECTORY: &str = "held";
/// What a temporary file's name ends with, until it is renamed.
const TEMPORARY: &str = ".tmp";

/// The held documents' directory.
#[derive(Debug, Clone)]
pub struct HeldFiles {
    dir: PathBuf,
}

impl HeldFiles {
    /// The `held` directory of `data_dir`, created if missing, readable by recto alone (0700).
    pub fn new(data_dir: &Path) -> io::Result<Self> {
        let files = Self::at(data_dir);
        fs::create_dir_all(&files.dir)?;
        fs::set_permissions(&files.dir, fs::Permissions::from_mode(0o700))?;
        Ok(files)
    }

    /// The `held` directory of `data_dir`, without creating it: for a server that cannot write
    /// there but must still remove what an earlier run left.
    pub fn at(data_dir: &Path) -> Self {
        Self {
            dir: data_dir.join(DIRECTORY),
        }
    }

    /// The directory, for log lines.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Runs `work` on the blocking thread pool, so the file system never stalls the runtime. A
    /// task that panicked is an error like any other.
    pub async fn blocking<T: Send + 'static>(
        self: &Arc<Self>,
        work: impl FnOnce(&HeldFiles) -> io::Result<T> + Send + 'static,
    ) -> io::Result<T> {
        let files = Arc::clone(self);
        tokio::task::spawn_blocking(move || work(&files))
            .await
            .unwrap_or_else(|error| Err(io::Error::other(error)))
    }

    /// Writes a document and returns the name it is kept under: 32 random hexadecimal digits.
    pub fn write(&self, bytes: &[u8]) -> io::Result<String> {
        let name = random_name();
        let temporary = self.dir.join(format!("{name}{TEMPORARY}"));
        let written = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temporary)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            fs::rename(&temporary, self.dir.join(&name))
        })();
        if let Err(error) = written {
            // Best effort: the orphan sweep removes it otherwise.
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        Ok(name)
    }

    /// The document kept under `file`.
    pub fn read(&self, file: &str) -> io::Result<Vec<u8>> {
        fs::read(self.path(file)?)
    }

    /// Removes the document kept under `file`; one already gone is not an error.
    pub fn remove(&self, file: &str) -> io::Result<()> {
        match fs::remove_file(self.path(file)?) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            removed => removed,
        }
    }

    /// The files no row names (`known`), temporary files included, last written before
    /// `written_before`. At startup nothing is being written, so every orphan goes; while recto
    /// runs, a document just written may not have its row yet, so only old ones do.
    pub fn orphans(
        &self,
        known: &HashSet<String>,
        written_before: SystemTime,
    ) -> io::Result<Vec<String>> {
        let entries = match fs::read_dir(&self.dir) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            entries => entries?,
        };
        let mut orphans = Vec::new();
        for entry in entries {
            let entry = entry?;
            // A name that is not UTF-8 was not written by recto, and cannot be removed by name.
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            // Renamed or removed since the listing: nothing to sweep.
            let metadata = match entry.metadata() {
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                metadata => metadata?,
            };
            if !metadata.is_file() || known.contains(&name) {
                continue;
            }
            if metadata.modified()? < written_before {
                orphans.push(name);
            }
        }
        orphans.sort();
        Ok(orphans)
    }

    /// The path of `file`, refused unless it is a plain name in the directory: a name from the
    /// database must not reach another directory.
    fn path(&self, file: &str) -> io::Result<PathBuf> {
        let plain =
            !file.is_empty() && file != "." && file != ".." && !file.contains(['/', '\\', '\0']);
        if plain {
            Ok(self.dir.join(file))
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "not the name of a held document",
            ))
        }
    }
}

/// 16 bytes from the system's random source, in hexadecimal.
fn random_name() -> String {
    let mut bytes = [0; 16];
    // The system's random source only fails on platforms recto does not run on.
    getrandom::fill(&mut bytes).expect("the system's random source works");
    HEXLOWER.encode(&bytes)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::testing::temporary_directory;

    #[test]
    fn documents_are_written_read_and_removed() {
        let data = temporary_directory();
        let files = HeldFiles::new(data.path()).unwrap();
        let mode = fs::metadata(files.dir()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
        let name = files.write(b"%PDF-1.7").unwrap();
        assert_eq!(name.len(), 32);
        assert!(name.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_ne!(files.write(b"%PDF-1.7").unwrap(), name);
        let mode = fs::metadata(files.dir().join(&name))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(files.read(&name).unwrap(), b"%PDF-1.7");
        files.remove(&name).unwrap();
        assert_eq!(
            files.read(&name).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        // Already gone.
        files.remove(&name).unwrap();
        // Creating it again keeps what is there.
        HeldFiles::new(data.path()).unwrap();
    }

    #[test]
    fn names_stay_in_the_directory() {
        let data = temporary_directory();
        let files = HeldFiles::new(data.path()).unwrap();
        fs::write(data.path().join("recto.db"), b"database").unwrap();
        for name in ["", ".", "..", "../recto.db", "a/b", "a\\b", "a\0b"] {
            assert_eq!(
                files.read(name).unwrap_err().kind(),
                io::ErrorKind::InvalidInput,
                "{name:?}"
            );
            assert_eq!(
                files.remove(name).unwrap_err().kind(),
                io::ErrorKind::InvalidInput,
                "{name:?}"
            );
        }
        assert!(data.path().join("recto.db").exists());
    }

    #[test]
    fn orphans_are_found() {
        let data = temporary_directory();
        let files = HeldFiles::new(data.path()).unwrap();
        for name in ["a", "b", "tmp.x"] {
            fs::write(files.dir().join(name), b"").unwrap();
        }
        fs::create_dir(files.dir().join("directory")).unwrap();
        let known = HashSet::from(["a".to_owned()]);
        let later = SystemTime::now() + Duration::from_secs(60);
        assert_eq!(files.orphans(&known, later).unwrap(), ["b", "tmp.x"]);
        // Files written since are left alone: their rows may be on their way.
        let earlier = SystemTime::now() - Duration::from_secs(3600);
        assert!(files.orphans(&known, earlier).unwrap().is_empty());
        // No directory, no orphans.
        let empty = temporary_directory();
        assert!(
            HeldFiles::at(empty.path())
                .orphans(&known, later)
                .unwrap()
                .is_empty()
        );
    }
}
