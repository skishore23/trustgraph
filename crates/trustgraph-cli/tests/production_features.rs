//! The shipped binary must never compile the engine's deliberate bugs.
//!
//! A unit test on `trustgraph_engine::FAULT_INJECTION` cannot guard this:
//! `cargo test --workspace` unifies features, so the constant is true in a
//! workspace test build even when the production graph is clean. Ask cargo
//! for the CLI's normal (non-dev) dependency graph instead.

use std::process::Command;

#[test]
fn cli_dependency_graph_has_no_fault_injection() {
    let Ok(cargo) = std::env::var("CARGO") else {
        eprintln!("skipped: CARGO is not set");
        return;
    };
    let output = match Command::new(cargo)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args([
            "tree",
            "-p",
            "trustgraph-cli",
            "-e",
            "normal,features",
            "--locked",
            "--offline",
        ])
        .output()
    {
        Ok(output) => output,
        Err(error) => {
            eprintln!("skipped: cannot run cargo: {error}");
            return;
        }
    };
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "cargo tree failed: {stderr}");
    let tree = String::from_utf8_lossy(&output.stdout);
    // The check means something only if the engine is in the graph at all.
    assert!(tree.contains("trustgraph-engine"), "{tree}");
    for forbidden in [
        "trustgraph-engine feature \"fault-injection\"",
        "trustgraph-sim feature \"fault-injection\"",
    ] {
        assert!(
            !tree.contains(forbidden),
            "the trustgraph binary's dependency graph enables {forbidden}"
        );
    }
}
