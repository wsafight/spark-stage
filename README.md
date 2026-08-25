# SparkStage

SparkStage turns an externally authored, validated shot contract into a resumable local video production workflow for DGX Spark and MiniMax H3.

The Rust control plane, Ratatui console, offline project portability, and non-GPU regression suite are implemented. MiniMax H3 workflow capabilities and DGX Spark performance remain unverified until hardware smoke tests and benchmark evidence are recorded.

Validate an externally authored script contract:

```bash
cargo run -- script validate skills/screenwriter/examples/valid-short-drama.json
```

Open the production console after starting a compatible SparkStage worker:

```bash
cargo run -- tui
```

Use `--socket PATH`, `--project PROJECT_ID`, and `--refresh-ms 1000` to override the worker connection. Inside the TUI, press `?` for the complete key map. Set `SPARKSTAGE_PLAYER` to a player executable such as `mpv`; paths are passed as a single argument without a shell.

Verify or move a project without starting the worker:

```bash
cargo run -- project verify --project PROJECT_ID --data-dir PATH
cargo run -- project export --project PROJECT_ID --output project.sparkstage.tar --data-dir PATH
cargo run -- project verify-archive --archive project.sparkstage.tar
cargo run -- project import --archive project.sparkstage.tar --data-dir OTHER_PATH
cargo run -- project migrate --project PROJECT_ID --data-dir PATH       # dry-run
```

Run the external-Agent contract fixtures and the full local quality gates:

```bash
sh scripts/evaluate-script-bundles.sh
cargo fmt --all --check
sh scripts/check-rust-file-size.sh
cargo test --all-targets
cargo clippy --all-targets --all-features -- -D warnings
cargo llvm-cov --all-targets --all-features --fail-under-lines 65
```

- [Product document](docs/product.md)
- [Technical design](docs/technical.md)
- [H3 optimization plan](docs/optimization.md)
- [P0 product feature checklist](docs/p0-features.md)

Rust is pinned to `1.98.0` with edition `2024`.
