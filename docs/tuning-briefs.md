# Tuning the briefs

Each stage's agent works from a brief in [`briefs/`](../briefs/): the skills in
`plugin/` and the prompts in `prompts/`, as its README describes. A `--prompts`
directory is laid out as `prompts/` is, such as `implement.md` and
`follow-ups/result-error.md`, and holds only the prompts it changes: the rest
are Linewright's own. Any other file in it is an error. The skills are built in,
so a change to one is tried by editing `briefs/plugin/` and rebuilding.

The parts in [`briefs/parts/`](../briefs/parts/) and the result contracts in
[`briefs/outputs/`](../briefs/outputs/) are Linewright's own wording of what it
fills into the prompts' placeholders, which no `--prompts` directory can
override. A change to a prompt's placeholders, which Linewright refuses unless
the code changes with it, is a change to the spec (`docs/specs/runs.md`,
"Contract").
