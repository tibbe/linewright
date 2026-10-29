# Tuning the briefs

Each stage's agent works from a brief in [`briefs/`](../briefs/): the skills
in `plugin/` and the prompts in `prompts/`, as its README describes. A
`--prompts` directory is laid out as `prompts/` is, such as `implement.md` and
`parts/facts.md`, and holds only the prompts it changes: the rest are
Linewright's own. Any other file in it is an error. The skills are built in, so
a change to one is tried by editing `briefs/plugin/` and rebuilding. To try a
change to a prompt against a real case:

```sh
lw capture owner/name#123 case/      # a spec change or an issue
lw --prompts briefs/prompts/ replay case/ --out run-1/
```

`run-1/` holds the issues and pull requests the run produced (`issues/`,
`pulls/`, as JSON, Markdown and diffs) and every session's brief, result and
transcript (`sessions/`).

The prompts are compiled into the binary, but no test depends on their wording:
tests check what Linewright fills into the placeholders, not the text around
them. The parts in `prompts/parts/` and `prompts/outputs/` are Linewright's own
wording of what it fills in, and `tests/integration/rendered_prompts.rs` holds
that byte for byte against `tests/integration/fixtures/prompts/`. A change to a
prompt's placeholders, which Linewright refuses unless the code changes with it,
is a change to the spec (`docs/specs/runs.md`, "Contract").
