#!/bin/sh
set -eu

cargo run --quiet -- script evaluate \
  --suite tests/fixtures/agent-script-bundles/expectations.json \
  --output target/script-bundle-evaluation.json
