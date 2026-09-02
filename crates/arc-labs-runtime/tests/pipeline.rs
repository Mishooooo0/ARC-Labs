//! **The Phase 5 acceptance gates**, end to end.
//!
//! Every test here runs against `MockLlm` with no network at all — which is the
//! point. A gate that needs Ollama running is a gate that does not run in CI,
//! and "the same graph twice produces identical output" is untestable against a
//! real model by definition.

use arc_labs_core::{Config, ModelAccess, Vault, VaultPath};
use arc_labs_ledger::Ledger;
use arc_labs_runtime::llm::{Cancel, MockLlm};
use arc_labs_runtime::run::{Event, RunError, Runner};

/// A vault with two notes and a four-node pipeline:
/// `query -> prompt -> transform -> note`.
fn fixture() -> (tempfile::TempDir, Vault, Ledger, arc_labs_index::Index) {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("source.md"),
        b"# Source\n\nThe ledger records provenance for every mutation.\n",
    )
    .unwrap();
    std::fs::write(tmp.path().join("target.md"), b"# Target\n\nExisting content.\n").unwrap();

    let canvas = concat!(
        "{\n\t\"nodes\":[\n",
        "\t\t{\"id\":\"q\",\"type\":\"text\",\"text\":\"provenance\",\"x\":0,\"y\":0,",
        "\"width\":300,\"height\":200,\"arc\":{\"kind\":\"query\",\"limit\":5}},\n",
        "\t\t{\"id\":\"p\",\"type\":\"text\",\"text\":\"Summarise: {{q}}\",\"x\":400,\"y\":0,",
        "\"width\":300,\"height\":200,\"arc\":{\"kind\":\"prompt\",\"model\":\"mock\"}},\n",
        "\t\t{\"id\":\"t\",\"type\":\"text\",\"text\":\"squeeze\",\"x\":800,\"y\":0,",
        "\"width\":300,\"height\":200,\"arc\":{\"kind\":\"transform\"}},\n",
        "\t\t{\"id\":\"out\",\"type\":\"file\",\"file\":\"target.md\",\"x\":1200,\"y\":0,",
        "\"width\":300,\"height\":200}\n",
        "\t],\n\t\"edges\":[\n",
        "\t\t{\"id\":\"e1\",\"fromNode\":\"q\",\"toNode\":\"p\"},\n",
        "\t\t{\"id\":\"e2\",\"fromNode\":\"p\",\"toNode\":\"t\"},\n",
        "\t\t{\"id\":\"e3\",\"fromNode\":\"t\",\"toNode\":\"out\"}\n",
        "\t]\n}"
    );
    std::fs::write(tmp.path().join("pipeline.canvas"), canvas.as_bytes()).unwrap();

    let vault = Vault::open(tmp.path()).unwrap();
    let ledger = Ledger::open(tmp.path()).unwrap();
    let mut index = arc_labs_index::Index::open_for_vault(tmp.path()).unwrap();
    index.build(&vault, false, |_| {}).unwrap();
    (tmp, vault, ledger, index)
}

fn config(endpoint: &str, access: ModelAccess) -> Config {
    let mut c = Config::default();
    c.model.endpoint = endpoint.into();
    c.model.access = access;
    c.model.instruct = "mock".into();
    c
}

fn canvas_path() -> VaultPath {
    VaultPath::new("pipeline.canvas").unwrap()
}

#[test]
fn a_four_node_pipeline_runs_end_to_end_offline() {
    let (tmp, vault, ledger, index) = fixture();
    let llm = MockLlm { chunks: 4, delay_ms: 0 };
    let cfg = config("http://localhost:11434", ModelAccess::LocalOnly);
    let runner = Runner {
        vault: &vault,
        index: Some(&index),
        ledger: &ledger,
        llm: &llm,
        config: &cfg,
        session: "run-1".into(),
    };

    let target = tmp.path().join("target.md");
    let before = std::fs::read(&target).unwrap();
    let mtime = std::fs::metadata(&target).unwrap().modified().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));

    let mut tokens = 0;
    let report = runner
        .run(&canvas_path(), "t", false, &Cancel::new(), &mut |e| {
            if matches!(e, Event::Token { .. }) {
                tokens += 1;
            }
        })
        .expect("the pipeline should run");

    // Three executable nodes ran, in dependency order.
    let kinds: Vec<&str> = report.results.iter().map(|r| r.kind.as_str()).collect();
    assert_eq!(kinds, ["query", "prompt", "transform"]);
    assert!(tokens > 1, "the prompt node should have streamed");
    assert!(report.total_tokens > 0, "tok/s needs a token count");

    // The query found the source note through the index.
    assert!(
        report.results[0].output.contains("provenance"),
        "query output was: {}",
        report.results[0].output
    );

    // Nothing left the machine.
    assert_eq!(report.egress_to, None);

    // **Constraint 4**: the target note is untouched, mtime included.
    assert_eq!(std::fs::read(&target).unwrap(), before, "the note was written to");
    assert_eq!(std::fs::metadata(&target).unwrap().modified().unwrap(), mtime, "mtime moved");

    // The output arrived as a proposal, attributed to the model and the card.
    let target_path = VaultPath::new("target.md").unwrap();
    let entries = ledger.read(&target_path).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].op, arc_labs_ledger::Op::Propose);
    assert!(entries[0].actor.is_agent(), "a pipeline writes as an agent");
    assert_eq!(entries[0].actor.model.as_deref(), Some("mock"));
    assert!(
        entries[0].actor.session.as_deref().unwrap_or_default().contains("run-1#t"),
        "the entry should name the run and the card, got {:?}",
        entries[0].actor.session
    );
    assert_eq!(report.results[2].proposed_to.as_deref(), Some("target.md"));
}

#[test]
fn every_run_records_its_cost() {
    let (_t, vault, ledger, index) = fixture();
    let llm = MockLlm { chunks: 3, delay_ms: 1 };
    let cfg = config("http://localhost:11434", ModelAccess::LocalOnly);
    let runner = Runner {
        vault: &vault,
        index: Some(&index),
        ledger: &ledger,
        llm: &llm,
        config: &cfg,
        session: "s".into(),
    };

    let report = runner.run(&canvas_path(), "t", false, &Cancel::new(), &mut |_| {}).unwrap();
    let prompt = report.results.iter().find(|r| r.kind == "prompt").unwrap();
    let cost = prompt.cost.expect("a prompt node must record its cost");
    assert!(cost.tokens > 0);
    assert!(cost.elapsed_ms > 0);
    assert!(cost.tokens_per_sec > 0.0, "tok/s is not optional on this hardware");
}

/// **The determinism gate.**
#[test]
fn the_same_graph_run_twice_produces_identical_output() {
    let (_t, vault, ledger, index) = fixture();
    let llm = MockLlm { chunks: 3, delay_ms: 0 };
    let cfg = config("http://localhost:11434", ModelAccess::LocalOnly);
    let runner = Runner {
        vault: &vault,
        index: Some(&index),
        ledger: &ledger,
        llm: &llm,
        config: &cfg,
        session: "s".into(),
    };

    let first = runner.run(&canvas_path(), "t", false, &Cancel::new(), &mut |_| {}).unwrap();
    for _ in 0..8 {
        let again = runner.run(&canvas_path(), "t", false, &Cancel::new(), &mut |_| {}).unwrap();
        let a: Vec<&String> = first.results.iter().map(|r| &r.output).collect();
        let b: Vec<&String> = again.results.iter().map(|r| &r.output).collect();
        assert_eq!(a, b, "output varied between runs");
    }
}

/// **The cancellation gate**: cancelling mid-stream leaves the target
/// byte-identical, with no half-made proposal.
#[test]
fn cancelling_mid_stream_leaves_the_target_byte_identical() {
    let (tmp, vault, ledger, index) = fixture();
    let llm = MockLlm { chunks: 60, delay_ms: 2 };
    let cfg = config("http://localhost:11434", ModelAccess::LocalOnly);
    let runner = Runner {
        vault: &vault,
        index: Some(&index),
        ledger: &ledger,
        llm: &llm,
        config: &cfg,
        session: "s".into(),
    };

    let target = tmp.path().join("target.md");
    let before = std::fs::read(&target).unwrap();
    let mtime = std::fs::metadata(&target).unwrap().modified().unwrap();

    let cancel = Cancel::new();
    let mut seen = 0;
    let result = runner.run(&canvas_path(), "t", false, &cancel, &mut |e| {
        if matches!(e, Event::Token { .. }) {
            seen += 1;
            if seen == 4 {
                cancel.cancel();
            }
        }
    });

    assert!(matches!(result, Err(RunError::Llm(_))), "a cancelled run should error");
    assert_eq!(std::fs::read(&target).unwrap(), before, "the note changed");
    assert_eq!(std::fs::metadata(&target).unwrap().modified().unwrap(), mtime, "mtime moved");
    // No proposal: the run never reached the transform or the file card.
    assert!(
        ledger.read(&VaultPath::new("target.md").unwrap()).unwrap().is_empty(),
        "a cancelled run left a proposal behind"
    );
}

// ── Egress ──────────────────────────────────────────────────────────────────

#[test]
fn local_only_blocks_a_remote_endpoint() {
    let (_t, vault, ledger, index) = fixture();
    let llm = MockLlm::default();
    let cfg = config("http://workstation.local:11434", ModelAccess::LocalOnly);
    let runner = Runner {
        vault: &vault,
        index: Some(&index),
        ledger: &ledger,
        llm: &llm,
        config: &cfg,
        session: "s".into(),
    };

    let err = runner.run(&canvas_path(), "t", false, &Cancel::new(), &mut |_| {}).unwrap_err();
    assert!(matches!(err, RunError::EgressBlocked(_)), "got {err:?}");
    // Nothing ran, so nothing was recorded.
    assert!(ledger.read(&canvas_path()).unwrap().is_empty());
}

#[test]
fn ask_each_run_refuses_until_approved_then_ledgers_the_egress() {
    let (_t, vault, ledger, index) = fixture();
    let llm = MockLlm::default();
    let cfg = config("http://workstation.local:11434", ModelAccess::AskEachRun);
    let runner = Runner {
        vault: &vault,
        index: Some(&index),
        ledger: &ledger,
        llm: &llm,
        config: &cfg,
        session: "s".into(),
    };

    let err = runner.run(&canvas_path(), "t", false, &Cancel::new(), &mut |_| {}).unwrap_err();
    assert!(matches!(err, RunError::EgressNeedsApproval(_)), "got {err:?}");

    // Approved: it runs, and the egress is recorded.
    let mut announced: Option<(String, u64)> = None;
    let report = runner
        .run(&canvas_path(), "t", true, &Cancel::new(), &mut |e| {
            if let Event::Egress { destination, bytes } = e {
                announced = Some((destination, bytes));
            }
        })
        .unwrap();

    assert_eq!(report.egress_to.as_deref(), Some("http://workstation.local:11434"));
    let (dest, bytes) = announced.expect("the surface must be told before bytes leave");
    assert_eq!(dest, "http://workstation.local:11434");
    assert!(bytes > 0);

    let entries = ledger.read(&canvas_path()).unwrap();
    let egress = entries
        .iter()
        .find(|e| e.op == arc_labs_ledger::Op::Egress)
        .expect("an egress entry must exist");
    assert_eq!(egress.destination.as_deref(), Some("http://workstation.local:11434"));
    assert_eq!(egress.bytes, Some(bytes));
    assert!(!egress.op.touches_file());
}

#[test]
fn a_trusted_endpoint_runs_without_asking_but_is_still_ledgered() {
    let (_t, vault, ledger, index) = fixture();
    let llm = MockLlm::default();
    let cfg = config("http://192.168.1.50:11434", ModelAccess::TrustedEndpoint);
    let runner = Runner {
        vault: &vault,
        index: Some(&index),
        ledger: &ledger,
        llm: &llm,
        config: &cfg,
        session: "s".into(),
    };

    let report = runner.run(&canvas_path(), "t", false, &Cancel::new(), &mut |_| {}).unwrap();
    assert_eq!(report.egress_to.as_deref(), Some("http://192.168.1.50:11434"));
    // Trusted means "do not ask", never "do not record".
    assert!(ledger
        .read(&canvas_path())
        .unwrap()
        .iter()
        .any(|e| e.op == arc_labs_ledger::Op::Egress));
}

#[test]
fn a_local_run_records_no_egress_at_all() {
    let (_t, vault, ledger, index) = fixture();
    let llm = MockLlm::default();
    let cfg = config("http://127.0.0.1:11434", ModelAccess::TrustedEndpoint);
    let runner = Runner {
        vault: &vault,
        index: Some(&index),
        ledger: &ledger,
        llm: &llm,
        config: &cfg,
        session: "s".into(),
    };

    runner.run(&canvas_path(), "t", false, &Cancel::new(), &mut |_| {}).unwrap();
    assert!(ledger
        .read(&canvas_path())
        .unwrap()
        .iter()
        .all(|e| e.op != arc_labs_ledger::Op::Egress));
}

#[test]
fn a_cycle_refuses_to_run_and_names_the_nodes() {
    let tmp = tempfile::tempdir().unwrap();
    let loop_canvas = concat!(
        "{\n\t\"nodes\":[\n",
        "\t\t{\"id\":\"a\",\"type\":\"text\",\"text\":\"x\",\"x\":0,\"y\":0,",
        "\"width\":10,\"height\":10,\"arc\":{\"kind\":\"prompt\"}},\n",
        "\t\t{\"id\":\"b\",\"type\":\"text\",\"text\":\"y\",\"x\":0,\"y\":0,",
        "\"width\":10,\"height\":10,\"arc\":{\"kind\":\"prompt\"}}\n",
        "\t],\n\t\"edges\":[\n",
        "\t\t{\"id\":\"e1\",\"fromNode\":\"a\",\"toNode\":\"b\"},\n",
        "\t\t{\"id\":\"e2\",\"fromNode\":\"b\",\"toNode\":\"a\"}\n",
        "\t]\n}"
    );
    std::fs::write(tmp.path().join("loop.canvas"), loop_canvas.as_bytes()).unwrap();

    let vault = Vault::open(tmp.path()).unwrap();
    let ledger = Ledger::open(tmp.path()).unwrap();
    let llm = MockLlm::default();
    let cfg = config("http://localhost:11434", ModelAccess::LocalOnly);
    let runner = Runner {
        vault: &vault,
        index: None,
        ledger: &ledger,
        llm: &llm,
        config: &cfg,
        session: "s".into(),
    };

    let err = runner
        .run(&VaultPath::new("loop.canvas").unwrap(), "a", false, &Cancel::new(), &mut |_| {})
        .unwrap_err();
    match err {
        RunError::Cycle(nodes) => assert_eq!(nodes, ["a", "b"]),
        other => panic!("expected a cycle, got {other:?}"),
    }
}

#[test]
fn running_one_node_does_not_run_the_rest_of_the_canvas() {
    let (_t, vault, ledger, index) = fixture();
    let llm = MockLlm::default();
    let cfg = config("http://localhost:11434", ModelAccess::LocalOnly);
    let runner = Runner {
        vault: &vault,
        index: Some(&index),
        ledger: &ledger,
        llm: &llm,
        config: &cfg,
        session: "s".into(),
    };

    // Target the query node: only it should run.
    let report = runner.run(&canvas_path(), "q", false, &Cancel::new(), &mut |_| {}).unwrap();
    assert_eq!(report.results.len(), 1);
    assert_eq!(report.results[0].kind, "query");
    assert_eq!(report.total_tokens, 0, "no model should have been called");
}
