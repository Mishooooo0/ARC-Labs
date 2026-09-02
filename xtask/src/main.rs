//! Build-side tooling. Not shipped.
//!
//! Three jobs, each of which turns a rule in the plan into a command that can
//! fail:
//!
//! - `gen-vault`   — the deterministic fixture vault every performance gate runs against
//! - `lint-tokens` — "a theme is a token swap, not a stylesheet fork", enforced
//! - `manifest` / `verify` — the byte-fidelity check behind the Phase 0 gate

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "xtask", about = "ARC-LABS build tooling")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Generate a deterministic fixture vault.
    GenVault {
        #[arg(long, default_value_t = 5000)]
        notes: usize,
        #[arg(long, default_value_t = 42)]
        seed: u64,
        /// Defaults outside the repo — and outside OneDrive — so the Phase 2
        /// file watcher never sees sync churn.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Fail if any colour literal exists outside the token file.
    LintTokens {
        #[arg(long, default_value = "ui/src")]
        dir: PathBuf,
    },
    /// Record every file's size and content hash.
    Manifest {
        #[arg(long)]
        vault: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
    /// Compare a vault against a manifest. Non-zero if a single byte moved.
    Verify {
        #[arg(long)]
        vault: PathBuf,
        #[arg(long)]
        manifest: PathBuf,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::GenVault { notes, seed, out } => gen_vault(notes, seed, out),
        Command::LintTokens { dir } => lint_tokens(&dir),
        Command::Manifest { vault, out } => write_manifest(&vault, &out),
        Command::Verify { vault, manifest } => verify(&vault, &manifest),
    }
}

// ── Deterministic RNG ───────────────────────────────────────────────────────

/// splitmix64. Hand-rolled rather than pulling in `rand`, because the fixture
/// must be reproducible *forever* — a `rand` upgrade that changes an algorithm
/// would silently change every performance baseline we ever recorded.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }
    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

const WORDS: &[&str] = &[
    "vault", "ledger", "canvas", "signal", "lattice", "anchor", "provenance", "index", "weave",
    "bench", "proposal", "actor", "hash", "graph", "runtime", "topology", "budget", "cursor",
    "gutter", "rail", "surface", "seam", "shell", "token", "fidelity", "embedding", "backlink",
];

const FOLDERS: &[&str] =
    &["Daily", "Projects", "Reference", "Archive", "Inbox", "Meetings", "Reading"];

// ── gen-vault ───────────────────────────────────────────────────────────────

fn default_fixture_dir() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(r"C:\arc-labs-fixtures\generated")
    } else {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        PathBuf::from(home).join("arc-labs-fixtures/generated")
    }
}

fn gen_vault(notes: usize, seed: u64, out: Option<PathBuf>) -> Result<()> {
    let root = out.unwrap_or_else(default_fixture_dir);
    if root.exists() {
        std::fs::remove_dir_all(&root)
            .with_context(|| format!("clearing {}", root.display()))?;
    }
    std::fs::create_dir_all(&root)?;
    for f in FOLDERS {
        std::fs::create_dir_all(root.join(f))?;
    }

    let mut rng = Rng(seed);
    let mut names: Vec<String> = Vec::with_capacity(notes);
    for i in 0..notes {
        let a = rng.pick(WORDS);
        let b = rng.pick(WORDS);
        names.push(format!("{a}-{b}-{i:05}"));
    }

    for (i, name) in names.iter().enumerate() {
        let folder = if rng.chance(70) { *rng.pick(FOLDERS) } else { "" };
        let rel =
            if folder.is_empty() { format!("{name}.md") } else { format!("{folder}/{name}.md") };
        let body = note_body(&mut rng, i, &names);

        // A realistic vault is not uniform. These proportions are what make the
        // fixture worth having: every one of them has broken a real markdown app.
        let bytes: Vec<u8> = if rng.chance(25) {
            // CRLF, as a Windows editor writes.
            body.replace('\n', "\r\n").into_bytes()
        } else if rng.chance(4) {
            // BOM, as Notepad writes.
            let mut v = vec![0xEF, 0xBB, 0xBF];
            v.extend_from_slice(body.as_bytes());
            v
        } else {
            body.into_bytes()
        };

        let path = root.join(&rel);
        std::fs::write(&path, &bytes).with_context(|| format!("writing {}", path.display()))?;
    }

    write_hard_cases(&root, &names)?;

    println!("generated {} notes in {}", notes, root.display());
    println!("seed {seed} — regenerating with the same seed produces identical bytes");
    Ok(())
}

fn note_body(rng: &mut Rng, i: usize, names: &[String]) -> String {
    let mut s = String::with_capacity(2048);

    if rng.chance(60) {
        // Frontmatter with keys deliberately out of alphabetical order: a YAML
        // round-trip would sort them, and the whole point is that we never do.
        s.push_str("---\n");
        s.push_str(&format!("zeta: {}\n", rng.below(1000)));
        s.push_str(&format!("title: {}\n", names[i]));
        s.push_str(&format!("tags: [{}, {}]\n", rng.pick(WORDS), rng.pick(WORDS)));
        s.push_str("alpha: 'quoted value'\n");
        s.push_str("---\n\n");
    }

    s.push_str(&format!("# {}\n\n", names[i].replace('-', " ")));

    let paragraphs = 2 + rng.below(8);
    for p in 0..paragraphs {
        if p > 0 && rng.chance(35) {
            s.push_str(&format!("## {}\n\n", rng.pick(WORDS)));
        }

        let sentences = 2 + rng.below(4);
        for _ in 0..sentences {
            let words = 8 + rng.below(14);
            for w in 0..words {
                if w > 0 {
                    s.push(' ');
                }
                // Links pointing at real notes, so backlinks are meaningful …
                if rng.chance(4) && !names.is_empty() {
                    let target = &names[rng.below(names.len())];
                    if rng.chance(30) {
                        s.push_str(&format!("[[{target}|{}]]", rng.pick(WORDS)));
                    } else {
                        s.push_str(&format!("[[{target}]]"));
                    }
                // … and some pointing nowhere, because real vaults have those.
                } else if rng.chance(1) {
                    s.push_str(&format!("[[missing-{}]]", rng.below(9999)));
                } else if rng.chance(3) {
                    s.push_str(&format!("#{}", rng.pick(WORDS)));
                } else {
                    s.push_str(rng.pick(WORDS));
                }
            }
            s.push_str(". ");
        }
        s.push_str("\n\n");

        if rng.chance(18) {
            // A fenced block containing wikilink and tag syntax. Anything that
            // regexes the raw document instead of walking the AST fails here.
            s.push_str("```rust\n");
            s.push_str("// [[NotALink]] and #nottag must survive verbatim\n");
            s.push_str(&format!("fn {}() -> usize {{ {} }}\n", rng.pick(WORDS), rng.below(100)));
            s.push_str("```\n\n");
        }
        if rng.chance(10) {
            s.push_str("| key | value |\n| --- | --- |\n");
            for _ in 0..1 + rng.below(4) {
                s.push_str(&format!("| {} | {} |\n", rng.pick(WORDS), rng.below(10000)));
            }
            s.push('\n');
        }
        if rng.chance(12) {
            s.push_str(&format!("- [ ] {}\n- [x] {}\n\n", rng.pick(WORDS), rng.pick(WORDS)));
        }
        if rng.chance(8) {
            s.push_str(&format!("![[{}]]\n\n", names[rng.below(names.len())]));
        }
    }
    s
}

/// The cases that only ever surface on one platform, or in one editor, written
/// explicitly so they fail loudly and early instead of in Phase 4.
fn write_hard_cases(root: &Path, names: &[String]) -> Result<()> {
    let edge = root.join("Edge");
    std::fs::create_dir_all(&edge)?;

    std::fs::write(edge.join("empty.md"), b"")?;
    std::fs::write(edge.join("no-trailing-newline.md"), b"# no newline at end")?;
    std::fs::write(edge.join("crlf.md"), b"# CRLF\r\n\r\nbody\r\n")?;
    std::fs::write(edge.join("lf.md"), b"# LF\n\nbody\n")?;
    std::fs::write(edge.join("mixed-endings.md"), b"# mixed\r\nsecond\nthird\r\n")?;
    std::fs::write(edge.join("bom.md"), b"\xEF\xBB\xBF# BOM\n\nbody\n")?;
    std::fs::write(edge.join("only-frontmatter.md"), b"---\ntitle: x\n---\n")?;
    std::fs::write(
        edge.join("code-fences.md"),
        b"# fences\n\n```\n[[NotALink]]\n#nottag\n```\n\nInline `[[AlsoNot]]` too.\n",
    )?;
    std::fs::write(
        edge.join("circular-a.md"),
        b"# A\n\npoints at [[circular-b]]\n",
    )?;
    std::fs::write(
        edge.join("circular-b.md"),
        b"# B\n\npoints back at [[circular-a]]\n",
    )?;
    std::fs::write(
        edge.join("unicode-\u{00e9}moji.md"),
        "# unicode\n\ncaf\u{00e9} \u{2615} \u{65e5}\u{672c}\u{8a9e}\n".as_bytes(),
    )?;
    std::fs::write(
        edge.join("html-in-note.md"),
        b"# raw html\n\n<script>alert(1)</script>\n\n<b>bold?</b>\n",
    )?;

    // Case-colliding names: legal on Linux, the same file on Windows. The write
    // is allowed to fail on Windows — that failure *is* the portability fact,
    // and the tree scan reports it either way.
    let _ = std::fs::write(edge.join("Case.md"), b"# upper\n");
    let _ = std::fs::write(edge.join("case.md"), b"# lower\n");

    // A canvas, so Phase 4 has generated input alongside the real fixtures.
    let a = names.first().map(String::as_str).unwrap_or("note");
    let b = names.get(1).map(String::as_str).unwrap_or("other");
    let canvas = format!(
        "{{\n\t\"nodes\":[\n\t\t{{\"id\":\"a1b2c3d4e5f60718\",\"x\":-100,\"y\":-100,\"width\":400,\"height\":400,\"type\":\"file\",\"file\":\"{a}.md\"}},\n\t\t{{\"id\":\"b2c3d4e5f6071829\",\"type\":\"file\",\"file\":\"{b}.md\",\"x\":400,\"y\":-100,\"width\":400,\"height\":400}}\n\t],\n\t\"edges\":[\n\t\t{{\"id\":\"c3d4e5f607182930\",\"fromNode\":\"a1b2c3d4e5f60718\",\"fromSide\":\"right\",\"toNode\":\"b2c3d4e5f6071829\",\"toSide\":\"left\"}}\n\t]\n}}"
    );
    std::fs::write(root.join("Projects/board.canvas"), canvas.as_bytes())?;
    Ok(())
}

// ── lint-tokens ─────────────────────────────────────────────────────────────

/// Fail on any colour literal outside the token file.
///
/// This is what makes "themes are a token swap, not a stylesheet fork" a fact
/// rather than an intention — and it is also what protects the reserved amber
/// and blue. A component that hardcodes `#d9a05b` has quietly spent the human
/// authorship colour on decoration, and constraint 6 dies by a thousand cuts.
fn lint_tokens(dir: &Path) -> Result<()> {
    let token_file = dir.join("lib/tokens.css");
    let mut findings: Vec<String> = Vec::new();

    for entry in walkdir::WalkDir::new(dir).into_iter().filter_map(Result::ok) {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if !matches!(ext, "css" | "svelte" | "ts") {
            continue;
        }
        if path == token_file {
            continue;
        }

        let text = std::fs::read_to_string(path)?;
        for (n, line) in text.lines().enumerate() {
            // Skip comment lines: prose about a colour is not a colour.
            let t = line.trim_start();
            if t.starts_with("//") || t.starts_with('*') || t.starts_with("/*") {
                continue;
            }
            if let Some(found) = colour_literal(line) {
                findings.push(format!("{}:{}  {}", path.display(), n + 1, found.trim()));
            }
        }
    }

    if findings.is_empty() {
        println!("lint-tokens: clean — every colour comes from lib/tokens.css");
        return Ok(());
    }
    eprintln!("lint-tokens: {} colour literal(s) outside the token file:\n", findings.len());
    for f in &findings {
        eprintln!("  {f}");
    }
    eprintln!("\nA theme is a token swap, not a stylesheet fork. Add a token instead.");
    bail!("{} colour literal(s) found", findings.len())
}

fn colour_literal(line: &str) -> Option<&str> {
    let bytes = line.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'#' {
            let rest = &line[i + 1..];
            let hex: String = rest.chars().take_while(|c| c.is_ascii_hexdigit()).collect();
            // 3, 4, 6 or 8 digits, and not part of a longer word.
            if matches!(hex.len(), 3 | 4 | 6 | 8) {
                let after = rest[hex.len()..].chars().next();
                if after.map(|c| !c.is_alphanumeric() && c != '_' && c != '-').unwrap_or(true) {
                    return Some(line);
                }
            }
        }
    }
    for f in ["rgb(", "rgba(", "hsl(", "hsla(", "oklch(", "color-mix("] {
        if line.contains(f) {
            return Some(line);
        }
    }
    None
}

// ── manifest / verify ───────────────────────────────────────────────────────

/// A vault's exact contents: path -> "size:hash".
///
/// Stronger than `git status`, and it works on a vault that is not a git repo —
/// which the real fixture is not. This is what proves the Phase 0 claim that an
/// hour of browsing leaves the vault untouched.
fn manifest_of(vault: &Path) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for entry in walkdir::WalkDir::new(vault).follow_links(false).into_iter().filter_map(Result::ok)
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(vault)?
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        let bytes = std::fs::read(entry.path())?;
        out.insert(rel, format!("{}:{}", bytes.len(), blake3::hash(&bytes).to_hex()));
    }
    Ok(out)
}

fn write_manifest(vault: &Path, out: &Path) -> Result<()> {
    let m = manifest_of(vault)?;
    let n = m.len();
    std::fs::write(out, serde_json::to_string_pretty(&m)?)?;
    println!("manifest: {n} files from {} -> {}", vault.display(), out.display());
    Ok(())
}

fn verify(vault: &Path, manifest: &Path) -> Result<()> {
    let before: BTreeMap<String, String> =
        serde_json::from_str(&std::fs::read_to_string(manifest)?)?;
    let after = manifest_of(vault)?;

    let mut problems = Vec::new();
    for (path, hash) in &before {
        match after.get(path) {
            None => problems.push(format!("REMOVED  {path}")),
            Some(h) if h != hash => problems.push(format!("CHANGED  {path}")),
            _ => {}
        }
    }
    for path in after.keys() {
        if !before.contains_key(path) {
            problems.push(format!("ADDED    {path}"));
        }
    }

    if problems.is_empty() {
        println!("verify: {} files, all byte-identical", after.len());
        return Ok(());
    }
    eprintln!("verify: the vault changed:\n");
    for p in &problems {
        eprintln!("  {p}");
    }
    bail!("{} file(s) differ from the manifest", problems.len())
}
