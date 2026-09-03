//! Drafting a template with a model.
//!
//! You describe the shape you want; a model writes a template; **you read it
//! before anything exists on disk**. That ordering is the whole point. Every
//! other place a model's words could reach a file in this product, they arrive
//! as a proposal — because nobody asked for them and nobody has seen them. Here
//! you asked, and you are looking at the result in the window, so what finally
//! lands is a human write with the model named in the reason.
//!
//! Nothing here is a dependency. No Ollama, no such model, or a timeout gives
//! the real reason and the rest of the creation window keeps working: drafting
//! is an accelerator, and an accelerator that takes the feature down with it
//! when it fails is worse than not having one.

use arc_labs_runtime::llm::{self, GenerateRequest, Ollama};
use arc_labs_runtime::Cancel;

use crate::{ApiError, ApiResult, ErrorCode};

/// What the model is told it is doing.
///
/// It asks for a *template* rather than a finished note — headings and
/// placeholders, not prose about a specific week. Without this the models
/// available on this hardware reliably return a filled-in example, which is a
/// note, not something you can reuse.
const SYSTEM: &str = "\
You write reusable note templates in Markdown for a personal notebook.

Rules:
- Output ONLY the template. No explanation, no preamble, no code fences.
- It is a SKELETON to be reused, not a filled-in example. Use headings, empty
  bullets and short prompts, never invented specifics.
- There are exactly THREE placeholders and no others: {{title}} is the note's
  name, {{date}} is today's date, {{time}} is the current time. Never invent a
  placeholder, and never use one as a list item, a step or a heading — a small
  model reliably writes a numbered step whose text is just {{date}} otherwise.
- Start with a single `# {{title}}` heading. Usually that is the only
  placeholder a template needs.
- Leave blanks for the writer to fill: an empty bullet, not a made-up example.
- No emoji. This notebook does not use them, and a template that arrives full
  of them is one the writer has to clean up before it is usable.
- Keep it short. A template someone has to delete half of is a bad template.

The template is for:";

impl crate::Api {
    /// Draft a template from a description. Returns the text; writes nothing.
    pub fn draft_template(&self, description: &str) -> ApiResult<String> {
        let description = description.trim();
        if description.is_empty() {
            return Err(ApiError::new(
                ErrorCode::Config,
                "describe the template you want",
            ));
        }

        let config = self.config();
        let endpoint = config.model.endpoint.clone();

        // The same rule the canvas runtime applies, reached through the same
        // helper — a second notion of "is this remote" is a second thing to get
        // wrong. See `Run::check_egress`.
        if !llm::is_local(&endpoint) {
            use arc_labs_core::ModelAccess;
            match config.model.access {
                ModelAccess::LocalOnly => {
                    return Err(ApiError::new(
                        ErrorCode::NotPermitted,
                        format!(
                            "{endpoint} is not on this machine, and model access is set to \
                             local-only. Change it in Settings → Models if you meant to allow it."
                        ),
                    ));
                }
                // Allowed. Deliberately *not* written to the ledger as an
                // egress entry: `Op::Egress` is keyed to a note and means "this
                // note's content left the machine". Drafting sends the sentence
                // you typed into the prompt box and no vault content at all, so
                // filing it against a note would be a false record — and a
                // ledger that logs things inaccurately is worse than one that
                // stays silent about what it does not cover.
                //
                // The guarantee that matters is enforced above: a remote
                // endpoint under local-only is refused outright, and the window
                // says where the description is going before you press Draft.
                ModelAccess::TrustedEndpoint | ModelAccess::AskEachRun => {}
            }
        }

        let prompt = format!("{SYSTEM} {description}\n");
        let mut req = GenerateRequest::new(&config.model.instruct, &prompt);
        // The default, deliberately, and NOT a smaller "a template is only a
        // page" number. The configured model is a reasoning model: it spends
        // tokens thinking before it writes anything, and a tight cap means the
        // budget is gone before the first line of output — which arrives as an
        // empty success, the most confusing possible result. `DEFAULT_MAX_TOKENS`
        // carries that lesson from Phase 5; overriding it downwards reproduced
        // the exact failure its doc comment warns about.
        req.max_tokens = GenerateRequest::DEFAULT_MAX_TOKENS;
        req.temperature = 0.4;
        // No reasoning. Writing a skeleton from a one-line description has
        // nothing to reason about, and the configured model otherwise spends its
        // whole budget thinking and returns nothing — measured at 62 seconds and
        // no output. Raising the cap does not help; it just thinks for longer.
        req.think = false;

        let backend = Ollama::new(&endpoint);
        let cancel = Cancel::new();
        let mut sink = |_: &str| {};

        let generated = llm::Llm::generate(&backend, &req, &cancel, &mut sink).map_err(|e| {
            ApiError::new(ErrorCode::Io, format!("could not draft a template: {e}"))
        })?;

        let text = clean(&generated.text);
        if text.trim().is_empty() {
            // A reasoning model that spent its whole budget thinking returns an
            // empty answer that otherwise looks like success. Say what happened
            // rather than handing back a blank template.
            return Err(ApiError::new(
                ErrorCode::Io,
                format!(
                    "{} returned nothing usable. A reasoning model can spend its \
                     whole budget before writing anything — try a smaller \
                     instruct model in Settings → Models.",
                    config.model.instruct
                ),
            ));
        }
        Ok(text)
    }
}

/// Strip what a model adds around the answer despite being asked not to.
///
/// Small local models wrap output in fences roughly half the time. Leaving them
/// in means every note created from the template starts with ```markdown.
fn clean(raw: &str) -> String {
    let mut text = raw.trim();

    if let Some(rest) = text.strip_prefix("```") {
        // Drop the language tag on the opening fence, if any.
        let rest = rest.split_once('\n').map(|(_, r)| r).unwrap_or(rest);
        text = rest.trim_end().strip_suffix("```").unwrap_or(rest).trim();
    }

    let mut out = text.to_string();
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fenced_output_is_unwrapped() {
        let raw = "```markdown\n# {{title}}\n\n## Notes\n```";
        assert_eq!(clean(raw), "# {{title}}\n\n## Notes\n");
    }

    /// Trailing whitespace goes with the fence. Harmless in a template, and it
    /// keeps generated files from carrying invisible cruft into every note made
    /// from them.
    #[test]
    fn trailing_whitespace_does_not_survive() {
        assert_eq!(clean("```\n# {{title}}\n\n-   \n```"), "# {{title}}\n\n-\n");
    }

    #[test]
    fn a_bare_fence_with_no_language_is_unwrapped_too() {
        assert_eq!(clean("```\n# {{title}}\n```"), "# {{title}}\n");
    }

    #[test]
    fn unfenced_output_is_left_alone_but_gains_a_trailing_newline() {
        assert_eq!(
            clean("# {{title}}\n\n## Notes"),
            "# {{title}}\n\n## Notes\n"
        );
    }

    #[test]
    fn an_empty_description_is_refused_before_any_request_is_made() {
        // No model call, no waiting to find out — the check is first in the
        // function precisely so a blank box costs nothing.
        let api = crate::Api::new(
            arc_labs_core::Config::default(),
            None,
            crate::Capabilities::desktop(),
        );
        assert_eq!(
            api.draft_template("   ").unwrap_err().code,
            ErrorCode::Config
        );
        assert_eq!(api.draft_template("").unwrap_err().code, ErrorCode::Config);
    }

    /// Local-only means local-only, and it is checked before any request goes
    /// out rather than after the reply comes back.
    #[test]
    fn a_remote_endpoint_is_refused_under_local_only() {
        let mut config = arc_labs_core::Config::default();
        config.model.endpoint = "http://198.51.100.7:11434".into();
        config.model.access = arc_labs_core::ModelAccess::LocalOnly;

        let api = crate::Api::new(config, None, crate::Capabilities::desktop());
        let err = api.draft_template("a weekly review").unwrap_err();

        assert_eq!(err.code, ErrorCode::NotPermitted);
        assert!(err.message.contains("local-only"), "got {}", err.message);
    }
}
