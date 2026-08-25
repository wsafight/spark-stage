---
name: screenwriter
description: Create or revise a SparkStage ScriptBundle from a short-film brief, validate it against the repository contract, and stop before video generation or project approval. Use for SparkStage story, bible, shot-list, dialogue, or prompt authoring; do not use for reviewing generated media or operating ComfyUI.
---

# SparkStage Screenwriter

Turn the user's brief into one schema-valid production contract. SparkStage owns validation and video execution; this skill does not operate a language-model runtime or a camera backend.

## Required Inputs

Read only:

- the user's brief and requested delivery specification;
- [the ScriptBundle schema](../../schemas/script-bundle.schema.json);
- [the valid short-drama example](examples/valid-short-drama.json) when a concrete shape is useful;
- [references/writing-rules.md](references/writing-rules.md) for short-drama content and continuity rules.

Do not inspect `raw/`, `refs/`, `final/`, unrelated projects, or user media unless the user explicitly authorizes that separate data access.

## Workflow

1. Draft the project, fictional adult characters, locations, story beats, and typed shots as one `ScriptBundle`. Work in a temporary file, never in the active project contract.
2. Keep creative descriptions in semantic fields. Never add ComfyUI node IDs, workflow JSON, model steps, scheduler, seed, or backend-specific parameters.
3. From the repository root, run `sparkstage script validate <bundle> --json`. During repository development, use `cargo run --quiet -- script validate <bundle> --json`.
4. Fix the exact JSON Pointer errors and validate again. Stop after two failed repair passes and report the remaining errors instead of silently weakening the story or schema.
5. Present a compact summary of the valid bundle. Do not run `script apply`, `script approve`, or any H3 command unless the user has explicitly asked to import or produce the project.

When the Agent host and model identity are available, write them to `authoring.agent_host` and `authoring.model`; never include API keys, account IDs, hidden reasoning, or the full chat transcript. Checked-in files under `tests/fixtures/agent-script-bundles/` are evaluation data, not extra authoring context: do not copy their story content into a user project.

## Output Boundary

Return or write exactly one JSON bundle. Preserve the original brief. All character and location references must use stable IDs, all on-screen characters must appear in each shot's `characters`, and dialogue must fit the shot duration budget. A valid bundle is ready for human approval, not automatically approved for filming.
