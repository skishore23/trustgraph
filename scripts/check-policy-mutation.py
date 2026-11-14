#!/usr/bin/env python3
"""Verify a permission regression is caught, without editing the real checkout."""
from pathlib import Path
import shutil
import subprocess
import tempfile

root = Path(__file__).resolve().parents[1]
with tempfile.TemporaryDirectory(prefix="trustgraph-mutation-") as directory:
    checkout = Path(directory) / "checkout"
    shutil.copytree(root, checkout, ignore=shutil.ignore_patterns("target", ".git", ".trustgraph"))
    source = checkout / "crates/trustgraph-core/src/verifier.rs"
    text = source.read_text()
    gate = "if !audience.recipients.is_subset(&closure.readers) {"
    assert text.count(gate) == 1, "permission gate moved; update this mutation intentionally"
    source.write_text(text.replace(gate, "if false {"))
    result = subprocess.run(
        ["cargo", "test", "--locked", "-p", "trustgraph-core", "--test", "governance",
         "broad_audience_is_blocked_by_transitive_private_source"],
        cwd=checkout, capture_output=True, text=True,
    )
    output = result.stdout + result.stderr
    print(output)
    assert result.returncode != 0 and "broad_audience_is_blocked_by_transitive_private_source ... FAILED" in output, "mutation was not caught by a behavioral assertion"
    print("Detected deliberate permission-gate bypass. Original checkout was untouched.")
