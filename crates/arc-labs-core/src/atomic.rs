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
        ".arc-write-{}-{}.tmp",
        std::process::id(),
        target.file_name().and_then(|n| n.to_str()).unwrap_or("note")
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
            assert_eq!(std::fs::read(&f).unwrap(), content, "bytes changed for {content:?}");
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
        assert!(strays.is_empty(), "left temp files after failure: {strays:?}");
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

        assert_eq!(reader.join().unwrap(), 0, "observed a partially written file");
    }
}
