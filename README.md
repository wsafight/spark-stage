# SparkStage

SparkStage turns an externally authored, validated shot contract into a resumable local video production workflow for DGX Spark and MiniMax H3.

The Rust control plane, Ratatui console, immutable reference assets, deterministic SRT/VTT delivery, external-Agent evaluation reports, local milestone hooks, offline project portability, and non-GPU regression suite are implemented. The checked-in MiniMax H3 T2V workflow is smoke-tested on this DGX Spark; I2V, FLF2V, R2V, and performance profiles remain unverified until separate hardware evidence is recorded.

Validate an externally authored script contract:

```bash
cargo run -- script validate skills/screenwriter/examples/valid-short-drama.json
```

Evaluate a checked-in or externally collected ScriptBundle suite:

```bash
cargo run -- script evaluate \
  --suite tests/fixtures/agent-script-bundles/expectations.json \
  --output target/script-bundle-evaluation.json
```

Manage immutable character or location references through the worker. Inspect impact before accepting invalidation of dependent takes and builds:

```bash
cargo run -- refs impact --project PROJECT_ID --kind character --id CHARACTER_ID
cargo run -- refs import --project PROJECT_ID --kind character --id CHARACTER_ID --file portrait.png
cargo run -- refs replace --project PROJECT_ID --reference REF_ID --file portrait-v2.png --accept-impact
cargo run -- refs verify --project PROJECT_ID
```

Every build with dialogue writes frozen `subtitles.srt` and `subtitles.vtt` files under its build directory and publishes matching delivery copies beside the draft, trailer, or final video.

Configure a local milestone hook for subsequent worker starts:

```bash
cargo run -- notifications default --output notifications.json
cargo run -- notifications validate --config notifications.json
cargo run -- notifications apply --config notifications.json --data-dir PATH
cargo run -- worker run --data-dir PATH
```

Enabled hooks require an absolute, executable, non-symlink regular file. SparkStage invokes it without a shell, clears the inherited environment, and sends the milestone JSON on stdin.

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
cargo test --all-targets --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo llvm-cov --all-targets --all-features --fail-under-lines 70
```

The current local non-GPU baseline is 243 tests (240 unit tests plus 3 CLI integration tests). The most recently recorded line coverage is 70.07%, with CI retaining a 70% gate; this change did not rerun coverage. Critical pure logic is held to a higher 85%+ target where practical. The T2V hardware claim is based on the separately recorded DGX smoke test, not the non-GPU suite.

- [Product document](docs/product.md)
- [Technical design](docs/technical.md)
- [H3 optimization plan](docs/optimization.md)
- [P0 product feature checklist](docs/p0-features.md)

Rust is pinned to `1.98.0` with edition `2024`.
