//! Templates.
//!
//! Ordinary notes in an ordinary folder. That is the whole design, and it is
//! deliberate: a template you can open, edit, link to and search is one you will
//! actually maintain, and Obsidian's own Templates plugin reads the same
//! convention — so a vault carrying templates works in both apps without
//! anything being moved or converted.
//!
//! The alternative was a hidden store under `.arc/`. It would keep the note tree
//! tidier, at the cost of templates being invisible to Obsidian, absent from the
//! vault's git history, and missing from a synced copy. For a product whose first
//! principle is that files are the source of truth, a hidden parallel store is
//! the wrong shape.

use arc_labs_core::VaultPath;

use crate::{ApiError, ApiResult, ErrorCode};

/// One template, as the picker shows it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Template {
    /// Vault path, so it can be opened and edited like any note.
    pub path: String,
    /// The file stem — what the picker lists.
    pub name: String,
}

/// Substitute the variables a template may carry.
///
/// **Obsidian's names, on purpose.** `{{title}}`, `{{date}}` and `{{time}}` are
/// what its Templates plugin substitutes, so a template written for one app
/// works unchanged in the other. Inventing a nicer syntax would make every
/// template a one-way door.
///
/// An unknown `{{placeholder}}` is left exactly as it is rather than blanked:
/// the author probably meant it as literal text, and silently deleting part of
/// someone's template is worse than leaving something for them to notice.
pub fn substitute(template: &str, title: &str, now: &str) -> String {
    // `now` is RFC3339: "2026-09-03T02:18:34Z".
    let date = now.split('T').next().unwrap_or(now);
    let time = now
        .split('T')
        .nth(1)
        .and_then(|t| t.get(..5))
        .unwrap_or("00:00");

    template
        .replace("{{title}}", title)
        .replace("{{date}}", date)
        .replace("{{time}}", time)
}

impl crate::Api {
    /// The templates folder, as a vault path.
    fn templates_root(&self) -> String {
        let folder = self.config().templates_folder;
        folder.trim_matches('/').to_string()
    }

    /// Every template in the vault, by name.
    ///
    /// An absent folder is not an error — it is the normal state of a vault that
    /// has never used a template, and the picker simply has nothing to offer.
    pub fn templates(&self) -> ApiResult<Vec<Template>> {
        let root = self.templates_root();
        if root.is_empty() {
            return Ok(Vec::new());
        }

        let prefix = format!("{root}/");
        let tree = self.tree()?;
        let mut out: Vec<Template> = tree
            .tree
            .entries
            .into_iter()
            .filter(|e| !e.is_dir)
            .filter(|e| e.path.as_str().starts_with(&prefix))
            .filter(|e| e.path.as_str().ends_with(".md"))
            .map(|e| Template {
                name: e.name.strip_suffix(".md").unwrap_or(&e.name).to_string(),
                path: e.path.as_str().to_string(),
            })
            .collect();

        out.sort_by_key(|t| t.name.to_lowercase());
        Ok(out)
    }

    /// Create a note from a template.
    ///
    /// Reads the template, substitutes, and goes through the ordinary
    /// `create_note` — so it is atomic, refuses to overwrite, and lands in the
    /// ledger exactly like any other note. A template is a starting point, not a
    /// special kind of file.
    pub fn create_note_from_template(
        &self,
        path: &VaultPath,
        template: &VaultPath,
    ) -> ApiResult<crate::NoteView> {
        let source = self.with_vault(|v| Ok(v.read_note(template)?))?;

        let title = path
            .as_str()
            .rsplit('/')
            .next()
            .and_then(|n| n.strip_suffix(".md"))
            .unwrap_or("Untitled")
            .to_string();

        let body = substitute(source.text(), &title, &arc_labs_ledger::now_rfc3339());
        self.create_note(path, &body)
    }

    /// Save text as a reusable template.
    ///
    /// Creates the templates folder on demand rather than at vault open: an
    /// empty `Templates/` in every vault that never uses one is litter.
    ///
    /// `drafted` names the model in the ledger entry. It is a **human** write
    /// either way — you asked for the draft, you read it in the window, you
    /// edited it and you pressed the button, which is the opposite of the
    /// proposal path and why this is not one. But "created" alone would lose
    /// the one fact worth keeping about where the words came from, and the
    /// caller is the only thing that knows it. The model's name comes from
    /// config rather than from the caller: a client should not get to write
    /// whatever it likes into the provenance record.
    pub fn save_template(&self, name: &str, body: &str, drafted: bool) -> ApiResult<Template> {
        let root = self.templates_root();
        if root.is_empty() {
            return Err(ApiError::new(
                ErrorCode::Config,
                "no templates folder is configured",
            ));
        }

        let stem = name.trim().trim_end_matches(".md").trim();
        if stem.is_empty() {
            return Err(ApiError::new(
                ErrorCode::InvalidPath,
                "a template needs a name",
            ));
        }

        let path = VaultPath::new(format!("{root}/{stem}.md")).map_err(ApiError::from)?;
        let reason = if drafted {
            format!(
                "drafted with {}, read and saved",
                self.config().model.instruct
            )
        } else {
            "created".to_string()
        };
        // `create_note` refuses to overwrite, so saving a template over an
        // existing one is an error the caller resolves rather than silent loss.
        self.create_note_because(&path, body, &reason)?;

        Ok(Template {
            path: path.as_str().to_string(),
            name: stem.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn obsidian_variables_are_substituted() {
        let out = substitute(
            "# {{title}}\n\nWritten {{date}} at {{time}}.\n",
            "Weekly review",
            "2026-09-03T02:18:34Z",
        );
        assert_eq!(out, "# Weekly review\n\nWritten 2026-09-03 at 02:18.\n");
    }

    /// Silently deleting part of someone's template is worse than leaving
    /// something they can see and fix.
    #[test]
    fn an_unknown_placeholder_is_left_alone() {
        let out = substitute(
            "{{title}} — {{project}} — {{nope}}",
            "A",
            "2026-09-03T02:18:34Z",
        );
        assert_eq!(out, "A — {{project}} — {{nope}}");
    }

    #[test]
    fn substitution_survives_a_malformed_timestamp() {
        // Never panics on a timestamp shape it did not expect.
        let out = substitute("{{date}}/{{time}}", "T", "not-a-timestamp");
        assert_eq!(out, "not-a-timestamp/00:00");
    }

    #[test]
    fn a_template_with_no_variables_is_copied_verbatim() {
        let body = "# Fixed\n\nNothing to substitute.\n";
        assert_eq!(substitute(body, "X", "2026-09-03T02:18:34Z"), body);
    }

    /// A drafted template is a **human** write — asked for, read, edited,
    /// saved — and the timeline has to say so in amber. But "created" alone
    /// would lose the one fact worth keeping about where the words came from,
    /// so the model is named in the reason. Both halves are asserted here
    /// because getting either one wrong is a provenance bug, and provenance is
    /// the reason this product exists.
    #[test]
    fn a_drafted_template_is_yours_and_names_the_model() {
        let tmp = tempfile::tempdir().unwrap();
        let mut config = arc_labs_core::Config::default();
        config.model.instruct = "qwen3.5:0.8b".into();

        let api = crate::Api::new(config, None, crate::Capabilities::desktop());
        api.open_vault(tmp.path()).unwrap();

        let t = api.save_template("Meeting", "# {{title}}\n", true).unwrap();
        assert_eq!(t.path, "Templates/Meeting.md");

        let entry = &api.timeline(&VaultPath::new(&t.path).unwrap()).unwrap()[0];
        assert_eq!(
            entry.actor_kind, "human",
            "you asked for it and you saved it"
        );
        assert_eq!(entry.op, "create");
        assert!(
            entry.reason.contains("qwen3.5:0.8b"),
            "the model belongs in the reason, got {:?}",
            entry.reason
        );

        // A template that was simply typed says nothing about a model, because
        // no model was involved. Naming one anyway would be the same lie in the
        // other direction.
        let plain = api.save_template("Typed", "# {{title}}\n", false).unwrap();
        let entry = &api.timeline(&VaultPath::new(&plain.path).unwrap()).unwrap()[0];
        assert_eq!(entry.reason, "created");
    }
}
