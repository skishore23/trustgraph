Test the TrustGraph hackathon workflow from this repository. The live service is
http://127.0.0.1:9080. Use the existing restricted agent token through the checked-in
CLI and helper. Do not print token contents or read the operator token.

1. Run `python3 scripts/hackathon-check.py --output /tmp/trustgraph-policy-report.json`.
   Report each failing case accurately. These checks must not activate policy,
   approve work, execute generated code or publish anything.
2. Run `cargo run --locked -p trustgraph-cli -- policy-simulate --case examples/payment-change.json --policy examples/payment-policy-expanded.json --expect allowed --output /tmp/trustgraph-expanded.json`.
   Explain why the active policy blocks the same two-file proposal that the
   candidate permits. Read the saved result rather than guessing the outcome.
3. Write a candidate JSON file at /tmp/trustgraph-documentation-policy.json with
   the supported schema: name "Documentation-only trial", allowed_paths
   ["README.md"], max_changed_files 1, require_tests true, approval_minutes 15.
   Compare examples/payment-fix.json against it with policy-simulate and
   `--expect blocked`. Explain the failure path.
4. Test examples/payment-stale.json with examples/payment-policy.json and
   `--expect blocked`. Explain why exact approval still fails for a changed patch
   even when the file is permitted.
5. Summarize active vs candidate decisions, HydraDB evidence IDs, and the fact
   that no live policy was activated. Mention that simulation uses hypothetical
   assumptions, while real execution separately runs a controlled Codex turn
   and executes Rust tests.

Stop after the simulator trial and give me this command for the real-agent phase:
`cargo run --locked -p trustgraph-cli -- run --goal "Fix valid_amount to accept zero, reject negatives and add a zero regression test. Preserve existing tests. Only change src/lib.rs."`

Do not approve, activate policies, push branches, create PRs, edit application
code or claim provenance for an independently created patch. The human operator
reviews the real broker-produced patch in the TrustGraph UI.
