# SparkStage

SparkStage turns an externally authored, validated shot contract into a resumable local video production workflow for DGX Spark and MiniMax H3.

Validate an externally authored script contract:

```bash
cargo run -- script validate skills/screenwriter/examples/valid-short-drama.json
```

Open the production console after starting a compatible SparkStage worker:

```bash
cargo run -- tui
```

Use `--socket PATH`, `--project PROJECT_ID`, and `--refresh-ms 1000` to override the worker connection. Inside the TUI, press `?` for the complete key map. Set `SPARKSTAGE_PLAYER` to a player executable such as `mpv`; paths are passed as a single argument without a shell.

- [Product document](docs/product.md)
- [Technical design](docs/technical.md)
- [H3 optimization plan](docs/optimization.md)

Rust is pinned to `1.98.0` with edition `2024`.
