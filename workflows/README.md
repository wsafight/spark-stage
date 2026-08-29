# MiniMax H3 ComfyUI workflow

SparkStage needs the ComfyUI **API-format** workflow used by the local MiniMax H3 installation. A normal UI workflow is not interchangeable with the API payload. The checked-in `minimax-h3-api.json` is the known DGX Spark node graph with project-specific prompt and references removed.

1. Open the workflow already known to generate video on the DGX Spark.
2. Export it in API format and save it as `workflows/minimax-h3-api.json`. Keep the H3 `length` input in **frames**; SparkStage converts requested seconds with the H3 17-frame alignment rule (5 seconds at 24 fps becomes 124 frames, 10 seconds becomes 243).
3. Run `sparkstage adapter scaffold` with explicit `prompt`, `seed`, `output_prefix`, and `output_node` mappings, or copy the example YAML for manual editing. The supplied `adapters/minimax-h3-comfy.yaml` maps the known H3 nodes and enables only the locally verified T2V operation.
4. Add optional mappings with repeated `--binding NAME=NODE.INPUT`; the command validates every declared node and input but never guesses them.
5. Add optional bindings only when they exist in this workflow. In particular, do not infer first-frame, last-frame, or reference-video support from the H3 model family name. H3 reference images use the `5.ref_images` binding; SparkStage uploads each project-relative image to ComfyUI, adds a `LoadImage` node at runtime, and submits links as `5.ref_images.ref_image_N` dot paths so ComfyUI V3 can rebuild the autogrow map. The base workflow must not contain fixed project references. H3 reference videos use the separate `ref_videos` autogrow input (IMAGE frame streams), so a plain file binding named `reference_video` is not valid.
6. Keep `verified_operations: []` until the bindings pass preflight and a minimal local smoke test succeeds. For the initial certification only, use a temporary config with `enabled: true`, start the worker with that config, and explicitly run `sparkstage shots smoke-test --accept-unverified`; add `--seed SEED` when comparing profiles so the tested change is isolated. Ordinary audition and final commands continue to reject unverified operations. The smoke test creates exactly one lineage-tracked take and never edits the adapter config.
7. Only after the smoke-test job is submitted, collected, probed, and saved successfully may `t2v` be added to `verified_operations` in the real config. Enable the real adapter at the same time. Repeat the evidence separately for I2V, FLF2V, and R2V.

The local T2V certification completed on 2026-08-29 with one production-worker take at 960x544, 24 fps, 124 frames, and 20 steps. The 5.167-second output contained H.264 video and 32 kHz AAC stereo audio, passed decode, duration, audio, black-frame, freeze, and silence checks, produced distinct boundary frames, and retained job, workflow, model, seed, and backend-job lineage. Its 455-second elapsed time is a smoke observation, not a performance baseline. A separate three-seed 12-step audition experiment averaged 267.7 seconds, but one seed had a 1.875-second static interval; the formal adapter therefore keeps the 20-step default. Full IDs and evidence are in `docs/evidence/h3-t2v-2026-08-29.md`.

Run the read-only check with:

```sh
cargo run -- preflight --adapter-config adapters/minimax-h3-comfy.yaml --json
```

The workflow can contain machine-specific model filenames and paths. Review it before committing it to a shared repository.
