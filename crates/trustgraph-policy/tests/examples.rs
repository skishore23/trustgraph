use std::collections::BTreeSet;
use std::path::PathBuf;

use trustgraph_policy::{Verdict, lint, parse, run_tests};

fn example_files() -> Vec<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/policies");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .map(|entry| entry.expect("directory entry").path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(".policy.yaml"))
        })
        .collect();
    files.sort();
    files
}

#[test]
fn every_example_is_clean() {
    let files = example_files();
    assert_eq!(files.len(), 5, "expected exactly five examples: {files:?}");

    for file in files {
        let name = file.display().to_string();
        let text = std::fs::read_to_string(&file).expect("example is readable");
        let parsed = parse(&text);
        assert!(
            parsed.diagnostics.is_empty(),
            "{name}: diagnostics {:?}",
            parsed.diagnostics
        );
        let policy = parsed.policy.expect("example parses");

        let lints = lint(&policy);
        assert!(lints.is_empty(), "{name}: lints {lints:?}");

        assert!(policy.tests.len() >= 3, "{name}: needs at least 3 tests");
        let results = run_tests(&policy);
        for r in &results {
            assert!(r.pass, "{name}: test `{}` failed: {:?}", r.name, r.outcome);
        }
        let expected: BTreeSet<Verdict> = policy.tests.iter().map(|c| c.expect).collect();
        for v in [Verdict::Allow, Verdict::Deny, Verdict::NeedsApproval] {
            assert!(expected.contains(&v), "{name}: no test expects {v:?}");
        }
    }
}
