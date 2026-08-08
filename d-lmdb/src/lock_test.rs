use super::DataDirLock;

#[test]
fn test_acquire_succeeds_on_fresh_dir() {
    let dir = tempfile::tempdir().unwrap();
    let lock = DataDirLock::acquire(dir.path());
    assert!(lock.is_ok());
}

#[test]
fn test_acquire_creates_dir_if_missing() {
    let parent = tempfile::tempdir().unwrap();
    let data_dir = parent.path().join("not-yet-created");
    assert!(!data_dir.exists());

    let lock = DataDirLock::acquire(&data_dir);

    assert!(lock.is_ok());
    assert!(data_dir.is_dir());
}

#[test]
fn test_second_acquire_on_same_dir_fails_while_first_held() {
    let dir = tempfile::tempdir().unwrap();
    let _first = DataDirLock::acquire(dir.path()).unwrap();
    let second = DataDirLock::acquire(dir.path());
    assert!(second.is_err());
}

#[test]
fn test_acquire_succeeds_again_after_first_lock_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let first = DataDirLock::acquire(dir.path()).unwrap();
    drop(first);
    let second = DataDirLock::acquire(dir.path());
    assert!(second.is_ok());
}
