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

The prompts are compiled into the binary, and tests quote their wording. After
changing a prompt, find the tests that quote the text you changed by searching
`tests/` for it, run their files first, and only then the full `scripts/check`:

```sh
grep -rlF 'the text you changed' tests/
cargo test --locked --test integration -- NAME:: NAME::  # each file found, without .rs
```
