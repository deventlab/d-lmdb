use super::DataDirLock;

/// Baseline: locking a directory nobody else is using must not fail. If this
/// breaks, every `DLmdb::open` call breaks with it.
#[test]
fn test_acquire_succeeds_on_fresh_dir() {
    let dir = tempfile::tempdir().unwrap();
    let lock = DataDirLock::acquire(dir.path());
    assert!(lock.is_ok());
}

/// `acquire` now runs before `LmdbStorageEngine`/`LmdbStateMachine` are
/// constructed (that's the whole point of this lock — see PR #11 review).
/// Those were the components that used to create `data_dir`. On a true first
/// run the directory doesn't exist yet, so `acquire` must create it itself,
/// or opening the lock file fails before anything else gets a chance to.
#[test]
fn test_acquire_creates_dir_if_missing() {
    let parent = tempfile::tempdir().unwrap();
    let data_dir = parent.path().join("not-yet-created");
    assert!(!data_dir.exists());

    let lock = DataDirLock::acquire(&data_dir);

    assert!(lock.is_ok());
    assert!(data_dir.is_dir());
}

/// The actual guarantee this lock exists to provide: two processes must not
/// both be able to touch the same `data_dir` at once. Without this, the bug
/// this lock fixes (LMDB files opened by two processes before either one
/// knows about the other) is back.
#[test]
fn test_second_acquire_on_same_dir_fails_while_first_held() {
    let dir = tempfile::tempdir().unwrap();
    let _first = DataDirLock::acquire(dir.path()).unwrap();
    let second = DataDirLock::acquire(dir.path());
    assert!(second.is_err());
}

/// The lock must not outlive the process that holds it. A legitimate restart
/// against the same `data_dir` (previous instance shut down cleanly) must be
/// able to proceed — otherwise every restart looks identical to two
/// processes fighting over the same directory, and nothing can ever restart.
#[test]
fn test_acquire_succeeds_again_after_first_lock_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let first = DataDirLock::acquire(dir.path()).unwrap();
    drop(first);
    let second = DataDirLock::acquire(dir.path());
    assert!(second.is_ok());
}
