//! Deciding what to do, and deciding it with no socket open.
//!
//! ## Why three manifests and not two
//!
//! A file that is here and not there is either one you made or one they
//! deleted. A two-way diff cannot tell those apart, and whichever way it
//! guesses is wrong half the time: guess "new" and every deletion comes back
//! from the dead on the next sync, guess "deleted" and creating a note on one
//! machine silently destroys it.
//!
//! So there is a third input — **base**, the state both sides last agreed on.
//! With it, "present locally, absent remotely" splits cleanly: absent from base
//! too means you made it, present in base means they deleted it.
//!
//! ## Why this file has no I/O
//!
//! Everything here is a pure function over three maps. That is not tidiness:
//! it is the only way this is testable at all. The interesting cases are a
//! delete racing an edit across two machines, and reproducing those against
//! real sockets and real clocks would be slow, flaky, and impossible to
//! enumerate. As values in a table they are eleven lines of test.
//!
//! ## What is never decided here
//!
//! Nothing is merged and nothing is overwritten. When both sides changed, the
//! answer is always [`Action::Conflict`] and a person looks at it. Automatic
//! merge of prose is how sync tools lose paragraphs.

use crate::manifest::Manifest;

/// One thing to do about one path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Local content is newer ground truth; send it.
    Push(String),
    /// Remote content changed and local did not; take it.
    Pull(String),
    /// Deleted here, untouched there; delete it there.
    PushDelete(String),
    /// Deleted there, untouched here; delete it here.
    PullDelete(String),
    /// Both sides moved. A person decides.
    Conflict { path: String, kind: Conflict },
}

/// Why a path could not be decided automatically.
///
/// The kinds are separate because the *question* put to the user differs. "Both
/// edited" asks which text to keep. "You deleted it, they edited it" asks
/// whether the deletion still stands now that someone has worked on it — which
/// is a different question with a different likely answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Conflict {
    /// Edited on both sides since the last agreement.
    BothEdited,
    /// Created independently on both sides, at the same path, with different
    /// content. There is no common ancestor at all.
    BothCreated,
    /// Deleted here, edited there.
    LocalDeletedRemoteEdited,
    /// Edited here, deleted there.
    LocalEditedRemoteDeleted,
}

impl Conflict {
    /// The wire name. One spelling of each kind, so a preview and a report
    /// cannot disagree about what to call the same situation.
    pub fn name(self) -> &'static str {
        match self {
            Conflict::BothEdited => "both-edited",
            Conflict::BothCreated => "both-created",
            Conflict::LocalDeletedRemoteEdited => "local-deleted-remote-edited",
            Conflict::LocalEditedRemoteDeleted => "local-edited-remote-deleted",
        }
    }
}

impl Action {
    pub fn path(&self) -> &str {
        match self {
            Action::Push(p)
            | Action::Pull(p)
            | Action::PushDelete(p)
            | Action::PullDelete(p)
            | Action::Conflict { path: p, .. } => p,
        }
    }

    pub fn is_conflict(&self) -> bool {
        matches!(self, Action::Conflict { .. })
    }
}

/// Work out what to do, given the last agreed state and both current ones.
///
/// Paths come out sorted, because `Manifest` is a `BTreeMap` and a plan whose
/// order changed run to run would be unreadable in a log and unassertable in a
/// test.
///
/// A path where nothing needs doing produces no action at all — including the
/// case where both sides changed to the *same* content, which is agreement
/// arrived at independently and not a conflict.
pub fn plan(base: &Manifest, local: &Manifest, remote: &Manifest) -> Vec<Action> {
    let mut paths: Vec<&String> = base
        .keys()
        .chain(local.keys())
        .chain(remote.keys())
        .collect();
    paths.sort();
    paths.dedup();

    let mut out = Vec::new();
    for path in paths {
        if let Some(action) = decide(path, base.get(path), local.get(path), remote.get(path)) {
            out.push(action);
        }
    }
    out
}

/// The whole truth table, for one path.
///
/// Written as one `match` on purpose. Every row is visible in one screen, and a
/// case nobody thought about is a compile error rather than a silent fallthrough
/// to "do nothing" — which for a sync engine means data quietly not arriving.
fn decide(
    path: &str,
    base: Option<&crate::manifest::FileState>,
    local: Option<&crate::manifest::FileState>,
    remote: Option<&crate::manifest::FileState>,
) -> Option<Action> {
    let p = || path.to_string();
    let conflict = |kind| {
        Some(Action::Conflict {
            path: path.to_string(),
            kind,
        })
    };

    match (base, local, remote) {
        // Never seen by either side. Not reachable via `plan`, which only
        // iterates paths that appear in at least one manifest.
        (None, None, None) => None,

        // No common ancestor.
        (None, Some(_), None) => Some(Action::Push(p())),
        (None, None, Some(_)) => Some(Action::Pull(p())),
        // Both invented the same path. Identical content is agreement, not a
        // conflict — two machines running the same template, or the same file
        // copied in twice. Asking a human to choose between two identical
        // documents is a question with no answer.
        (None, Some(l), Some(r)) => (l != r).then(|| conflict(Conflict::BothCreated)).flatten(),

        // Known to both, still on both.
        (Some(b), Some(l), Some(r)) => {
            if l == r {
                // Already agree. The base may be stale, and that is harmless —
                // the caller advances it after a successful pass.
                None
            } else if l == b {
                Some(Action::Pull(p()))
            } else if r == b {
                Some(Action::Push(p()))
            } else {
                conflict(Conflict::BothEdited)
            }
        }

        // Gone from one side.
        (Some(b), None, Some(r)) => {
            if r == b {
                Some(Action::PushDelete(p()))
            } else {
                // They worked on the thing you threw away. Deleting it now
                // would destroy work that was never on this machine.
                conflict(Conflict::LocalDeletedRemoteEdited)
            }
        }
        (Some(b), Some(l), None) => {
            if l == b {
                Some(Action::PullDelete(p()))
            } else {
                conflict(Conflict::LocalEditedRemoteDeleted)
            }
        }

        // Both deleted it. Agreement; the caller drops it from the base.
        (Some(_), None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{FileState, Manifest};

    /// Distinct states, so `a != b` in a test means what it says.
    fn s(tag: &str) -> FileState {
        FileState {
            len: tag.len() as u64,
            hash: blake3::hash(tag.as_bytes()).to_hex().to_string(),
        }
    }

    fn m(entries: &[(&str, &str)]) -> Manifest {
        entries
            .iter()
            .map(|(p, tag)| ((*p).to_string(), s(tag)))
            .collect()
    }

    /// One row of the table per assertion, in the order the module doc lists
    /// them. This is the test that matters most in the crate: every one of
    /// these is a way to lose someone's notes.
    #[test]
    fn the_truth_table() {
        let none = Manifest::new();

        // ── no common ancestor ──────────────────────────────────────────────
        assert_eq!(
            plan(&none, &m(&[("A", "v1")]), &none),
            vec![Action::Push("A".into())],
            "made here, never seen there"
        );
        assert_eq!(
            plan(&none, &none, &m(&[("A", "v1")])),
            vec![Action::Pull("A".into())],
            "made there, never seen here"
        );
        assert_eq!(
            plan(&none, &m(&[("A", "v1")]), &m(&[("A", "v2")])),
            vec![Action::Conflict {
                path: "A".into(),
                kind: Conflict::BothCreated
            }],
            "same path invented twice with different content"
        );
        assert_eq!(
            plan(&none, &m(&[("A", "same")]), &m(&[("A", "same")])),
            vec![],
            "same path invented twice with the SAME content is agreement"
        );

        // ── known to both ───────────────────────────────────────────────────
        let base = m(&[("A", "v1")]);
        assert_eq!(
            plan(&base, &m(&[("A", "v1")]), &m(&[("A", "v1")])),
            vec![],
            "nobody touched it"
        );
        assert_eq!(
            plan(&base, &m(&[("A", "v2")]), &m(&[("A", "v1")])),
            vec![Action::Push("A".into())],
            "only this side moved"
        );
        assert_eq!(
            plan(&base, &m(&[("A", "v1")]), &m(&[("A", "v2")])),
            vec![Action::Pull("A".into())],
            "only that side moved"
        );
        assert_eq!(
            plan(&base, &m(&[("A", "v2")]), &m(&[("A", "v3")])),
            vec![Action::Conflict {
                path: "A".into(),
                kind: Conflict::BothEdited
            }],
            "both moved, differently"
        );
        assert_eq!(
            plan(&base, &m(&[("A", "v2")]), &m(&[("A", "v2")])),
            vec![],
            "both moved to the same place, which is agreement"
        );

        // ── gone from one side ──────────────────────────────────────────────
        assert_eq!(
            plan(&base, &none, &m(&[("A", "v1")])),
            vec![Action::PushDelete("A".into())],
            "deleted here, untouched there"
        );
        assert_eq!(
            plan(&base, &m(&[("A", "v1")]), &none),
            vec![Action::PullDelete("A".into())],
            "deleted there, untouched here"
        );
        assert_eq!(
            plan(&base, &none, &m(&[("A", "v2")])),
            vec![Action::Conflict {
                path: "A".into(),
                kind: Conflict::LocalDeletedRemoteEdited
            }],
            "they worked on what this machine threw away"
        );
        assert_eq!(
            plan(&base, &m(&[("A", "v2")]), &none),
            vec![Action::Conflict {
                path: "A".into(),
                kind: Conflict::LocalEditedRemoteDeleted
            }],
            "this machine worked on what they threw away"
        );
        assert_eq!(
            plan(&base, &none, &none),
            vec![],
            "both deleted it; nothing to do"
        );
    }

    /// The failure a two-manifest diff cannot avoid, stated as a test.
    ///
    /// Without a base, "here and not there" has to be guessed. Guess "new" and
    /// this deletion is undone on the next sync, for ever.
    #[test]
    fn a_deletion_is_not_resurrected_on_the_next_pass() {
        let base = m(&[("Gone", "v1")]);
        // First pass: deleted here, so the hub is told to delete it.
        assert_eq!(
            plan(&base, &Manifest::new(), &m(&[("Gone", "v1")])),
            vec![Action::PushDelete("Gone".into())]
        );
        // The pass succeeded, so the base no longer holds it. Second pass has
        // nothing to say — rather than seeing it on neither side and pulling it
        // back from somewhere.
        assert_eq!(
            plan(&Manifest::new(), &Manifest::new(), &Manifest::new()),
            vec![]
        );
    }

    /// A lost or first-run base makes everything differing a conflict. Noisy,
    /// and never wrong: with no record of what was agreed, there is nothing to
    /// justify choosing one side over the other.
    #[test]
    fn no_base_at_all_never_silently_picks_a_winner() {
        let local = m(&[("Same", "x"), ("Differs", "here"), ("OnlyHere", "h")]);
        let remote = m(&[("Same", "x"), ("Differs", "there"), ("OnlyThere", "t")]);

        let actions = plan(&Manifest::new(), &local, &remote);
        assert_eq!(
            actions,
            vec![
                Action::Conflict {
                    path: "Differs".into(),
                    kind: Conflict::BothCreated
                },
                Action::Push("OnlyHere".into()),
                Action::Pull("OnlyThere".into()),
            ]
        );
        // "Same" is absent: identical on both sides needs no transfer.
        assert!(!actions.iter().any(|a| a.path() == "Same"));
    }

    #[test]
    fn a_plan_is_in_a_stable_sorted_order() {
        let local = m(&[("z.md", "1"), ("a.md", "1"), ("m/n.md", "1")]);
        let actions = plan(&Manifest::new(), &local, &Manifest::new());
        let paths: Vec<&str> = actions.iter().map(|a| a.path()).collect();
        assert_eq!(paths, vec!["a.md", "m/n.md", "z.md"]);
    }

    #[test]
    fn nothing_to_do_is_an_empty_plan() {
        let both = m(&[("A", "v1"), ("B", "v2")]);
        assert!(plan(&both, &both, &both).is_empty());
    }

    /// Whatever else changes, this must hold: a plan never both sends and
    /// takes the same path, and never silently resolves a conflict.
    #[test]
    fn one_action_per_path_at_most() {
        let base = m(&[("A", "v1"), ("B", "v1"), ("C", "v1")]);
        let local = m(&[("A", "v2"), ("B", "v1"), ("D", "new")]);
        let remote = m(&[("A", "v3"), ("C", "v2"), ("E", "new")]);

        let actions = plan(&base, &local, &remote);
        let mut seen: Vec<&str> = actions.iter().map(|a| a.path()).collect();
        let before = seen.len();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), before, "a path got two actions");
    }
}
