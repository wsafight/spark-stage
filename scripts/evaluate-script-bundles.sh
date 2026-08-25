#!/bin/sh
set -eu

cargo test --lib validation::tests::external_agent_script_bundles_match_checked_in_expectations -- --exact
