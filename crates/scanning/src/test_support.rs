use tempfile::TempDir;

/// Under the target directory, never the system temp dir.
pub fn temp_dir() -> TempDir {
    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-tmp");
    std::fs::create_dir_all(&base).unwrap();
    TempDir::new_in(base).unwrap()
}
