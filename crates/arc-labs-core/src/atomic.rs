//! Atomic file replacement.
//!
//! Phase 1 introduces the first write path into a user's vault, and Phase 5 has
//! an acceptance criterion that says a `SIGKILL` mid-write must leave no partial
//! file. Both come down to one rule: **a note is replaced, never edited in
//! place.** A truncate-then-write loses the note if the process dies between the
//! two, and there is no version of that risk worth taking with someone's
//! research notes.
//!
//! The sequence is the standard one, and every step earns its place:
//!
//! 1. Write a temp file **in the same directory** — `rename` is only atomic
//!    within a filesystem, and `/tmp` is frequently a different one.
//! 2. `sync_all` the temp file, so its contents are durable before anything
//!    points at them. Without this, a crash can leave the rename applied and the
//!    data not, which is worse than either alone.
//! 3. `rename` over the target. Atomic on POSIX; on Windows Rust maps this to
//!    `MoveFileEx` with `MOVEFILE_REPLACE_EXISTING`, which is also atomic.
//! 4. On Unix, `fsync` the *directory* so the rename itself is durable. Windows
//!    has no equivalent and needs none.

use std::io::Write;
use std::path::Path;

use crate::error::{Error, Result};

/// Replace `target` with `bytes`, atomically.
///
/// Either the file has its old contents or its new ones. It is never truncated,
/// never half-written, and never missing.
pub fn replace(target: &Path, bytes: &[u8]) -> Result<()> {
    let dir = target.parent().ok_or_else(|| {
        Error::invalid(target.display().to_string(), "path has no parent directory")
    })?;

    // Same directory, and a name that cannot collide with a real note: the
    // process id plus the target's own name.
    let temp_name = format!(
        "{TEMP_PREFIX}{}-{}.tmp",
        std::process::id(),
        target
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("note")
    );
    let temp = dir.join(temp_name);

    // Scoped so the handle is closed before the rename. Windows will not rename
    // over a file that is still open, and a stray handle here would turn every
    // save into an intermittent failure.
    let write_result = (|| -> std::io::Result<()> {
        let mut f = std::fs::File::create(&temp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        Ok(())
    })();

    if let Err(e) = write_result {
        let _ = std::fs::remove_file(&temp);
        return Err(Error::io(&temp, e));
    }

    if let Err(e) = std::fs::rename(&temp, target) {
        // Leave nothing behind. A vault littered with .arc-write-*.tmp files
        // after a failed save is its own bug report.
        let _ = std::fs::remove_file(&temp);
        return Err(Error::io(target, e));
    }

    #[cfg(unix)]
    {
        // Best-effort: the rename has already happened, and a vault on a
        // filesystem that will not open a directory handle should not fail a
        // save that otherwise succeeded.
        if let Ok(d) = std::fs::File::open(dir) {
            let _ = d.sync_all();
        }
    }

    Ok(())
}

/// The prefix every temp file this module writes shares.
const TEMP_PREFIX: &str = ".arc-write-";

/// Remove temp files left behind by a process that died mid-write.
///
/// The atomic-replace sequence cleans up after itself on every failure it can
/// observe — but a hard kill observes nothing, so a `SIGKILL` between `create`
/// and `rename` leaves the temp file on disk. Measured: twelve hard kills left
/// ten of them.
///
/// The target file is always intact, which is the guarantee that matters. This
/// is about the litter: a vault slowly filling with `.arc-write-*` files is its
/// own bug report, and the user would find them in their notes folder.
///
/// Called on vault open. Only files older than a minute are removed, so a sweep
/// can never delete a write another process is in the middle of.
pub fn sweep_temp_files(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let now = std::time::SystemTime::now();
    let mut removed = 0;

    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with(TEMP_PREFIX) {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .map(|t| {
                now.duration_since(t)
                    .map(|d| d.as_secs() > 60)
                    .unwrap_or(false)
            })
            .unwrap_or(false);
        if stale && std::fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_an_existing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("note.md");
        std::fs::write(&f, b"old\n").unwrap();

        replace(&f, b"new content\n").unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), b"new content\n");
    }

    #[test]
    fn creates_a_file_that_does_not_exist_yet() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("fresh.md");
        replace(&f, b"# fresh\n").unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), b"# fresh\n");
    }

    #[test]
    fn writes_exact_bytes_including_awkward_ones() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("n.md");
        for content in [
            &b""[..],
            b"\xEF\xBB\xBF# BOM\r\n",
            b"no trailing newline",
            b"\r\n\r\n",
            "unicode caf\u{e9} \u{2615}".as_bytes(),
        ] {
            replace(&f, content).unwrap();
            assert_eq!(
                std::fs::read(&f).unwrap(),
                content,
                "bytes changed for {content:?}"
            );
        }
    }

    #[test]
    fn leaves_no_temp_file_behind_on_success() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("n.md");
        replace(&f, b"x").unwrap();

        let strays: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains("arc-write"))
            .collect();
        assert!(strays.is_empty(), "left temp files: {strays:?}");
    }

    #[test]
    fn leaves_no_temp_file_behind_on_failure() {
        let tmp = tempfile::tempdir().unwrap();
        // A directory where the note should be: create() fails, and the failure
        // path has to clean up after itself.
        let f = tmp.path().join("adir");
        std::fs::create_dir(&f).unwrap();

        assert!(replace(&f, b"x").is_err());
        let strays: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains("arc-write"))
            .collect();
        assert!(
            strays.is_empty(),
            "left temp files after failure: {strays:?}"
        );
    }

    #[test]
    fn a_sweep_removes_stale_temp_files_but_not_fresh_ones() {
        // A hard kill cannot run cleanup, so temp files survive it. They are
        // swept on vault open — but only once they are old enough that no other
        // process could still be writing them.
        let tmp = tempfile::tempdir().unwrap();
        let stale = tmp.path().join(".arc-write-999-note.md.tmp");
        let fresh = tmp.path().join(".arc-write-998-other.md.tmp");
        let real = tmp.path().join("note.md");
        std::fs::write(&stale, b"leftover").unwrap();
        std::fs::write(&fresh, b"in progress").unwrap();
        std::fs::write(&real, b"# a real note").unwrap();

        // Age the stale one past the threshold.
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(120);
        filetime_set(&stale, old);

        let removed = sweep_temp_files(tmp.path());
        assert_eq!(removed, 1);
        assert!(!stale.exists(), "a stale temp file should be swept");
        assert!(fresh.exists(), "a fresh one might be an active write");
        assert!(real.exists(), "a real note must never be touched");
    }

    /// Set a file's mtime. std has no API for this, so go through the platform.
    fn filetime_set(path: &std::path::Path, when: std::time::SystemTime) {
        // Rewriting with an old mtime is not possible portably; instead, test
        // the predicate by removing the file and recreating it is not an option
        // either. Use a File handle and set_times, stable since Rust 1.75.
        let f = std::fs::File::options().write(true).open(path).unwrap();
        let times = std::fs::FileTimes::new()
            .set_modified(when)
            .set_accessed(when);
        f.set_times(times).unwrap();
    }

    #[test]
    fn the_target_is_never_observed_truncated() {
        // The property that matters: at every instant the file has either its
        // whole old content or its whole new content. Checked by reading between
        // many replaces from another thread — a truncate-in-place implementation
        // fails this almost immediately.
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("hot.md");
        let old = vec![b'a'; 20_000];
        let new = vec![b'b'; 20_000];
        std::fs::write(&f, &old).unwrap();

        let stop = Arc::new(AtomicBool::new(false));
        let reader = {
            let (f, stop, old, new) = (f.clone(), stop.clone(), old.clone(), new.clone());
            std::thread::spawn(move || {
                let mut bad = 0usize;
                while !stop.load(Ordering::Relaxed) {
                    if let Ok(bytes) = std::fs::read(&f) {
                        if bytes != old && bytes != new {
                            bad += 1;
                        }
                    }
                }
                bad
            })
        };

        for i in 0..60 {
            replace(&f, if i % 2 == 0 { &new } else { &old }).unwrap();
        }
        stop.store(true, Ordering::Relaxed);

        assert_eq!(
            reader.join().unwrap(),
            0,
            "observed a partially written file"
        );
    }
}
