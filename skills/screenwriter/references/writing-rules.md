# SparkStage Short-Drama Writing Rules

Use these rules when the selected pipeline is `short-drama`.

## Content

- Use fictional characters who are explicitly at least 18 years old.
- Do not imitate real public figures or write minors, school uniforms, children's clothing, or graphic harm.
- Keep the story within the user's stated boundaries and mark all supplied material as user-provided rather than inferring rights.

## Structure

- Match `project.shot_count` and `project.target_duration_seconds` exactly.
- Keep each shot at 15 seconds or less.
- Use stable lowercase IDs for characters and locations, and `S01`, `S02`, ... for shots.
- List every on-screen character in `shot.characters`, including silent characters.
- Reference only declared character and location IDs.
- Keep delivery width, height, and fps identical across shots.

## Dialogue And Continuity

- Prefer short spoken lines with breathing room at the start and end of each shot.
- Use `camera.screen_direction` to maintain the established 180-degree axis.
- For continuous shots, make `state_in` agree with the referenced shot's `state_out` for shared state keys.
- Use `conditioning` only when the selected operation requires it: none for `t2v`, first frame for `i2v`, first and last frames for `flf2v`, and reference images or video for `r2v`.

## Prompt

Describe visible action, framing, performance, environment, and exclusions. Do not put execution controls, model names, seeds, sampler settings, or workflow node data in the prompt or any other shot field.
