#!/bin/sh
set -eu
cd "$(dirname "$0")/../.."
cargo build --locked -p trustgraph-cli --example fake_upstream
cargo test --locked -p trustgraph-cli --test gateway protected_gateway_demo -- --nocapture
