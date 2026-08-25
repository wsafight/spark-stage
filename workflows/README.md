# MiniMax H3 ComfyUI workflow

SparkStage needs the ComfyUI **API-format** workflow used by the local MiniMax H3 installation. A normal UI workflow is not interchangeable with the API payload.

1. Open the workflow already known to generate video on the DGX Spark.
2. Export it in API format and save it as `workflows/minimax-h3-api.json`.
3. Run `sparkstage adapter scaffold` with explicit `prompt`, `seed`, `output_prefix`, and `output_node` mappings, or copy the example YAML for manual editing.
4. Add optional mappings with repeated `--binding NAME=NODE.INPUT`; the command validates every declared node and input but never guesses them.
5. Add optional bindings only when they exist in this workflow. In particular, do not infer first-frame, last-frame, or reference-video support from the H3 model family name.
6. Keep `enabled: false` and `verified_operations: []` until the bindings pass preflight and a minimal local smoke test succeeds.
7. After the T2V smoke test, add `t2v` to `verified_operations` and enable the adapter. Repeat the evidence separately for I2V, FLF2V, and R2V.

Run the read-only check with:

```sh
cargo run -- preflight --adapter-config adapters/minimax-h3-comfy.yaml --json
```

The workflow can contain machine-specific model filenames and paths. Review it before committing it to a shared repository.
