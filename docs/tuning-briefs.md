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

## Capturing a case

A triage, design or implement run that went badly is worth keeping as a case,
to try a change to its stage's brief against (`docs/specs/benchmarks.md`). Find
the run's ID with `lw runs N`, such as `43/implement-2`, and capture it under a
name of lowercase letters, digits and hyphens, with what a good result would
have been:

```sh
lw capture 43/implement-2 43-implement-2 --criteria good-result.md
```

It works whether `serve` is running or not, with the token `serve` uses: the
commits are fetched from GitHub. The case is written to `cases/43-implement-2/`
under the work directory:

```text
case.json        the stage, the item, the start, the run and the API version
items.json       the items the run was given, as GitHub's REST API gave them
repository.git/  a bare repository of the branch heads it was given
criteria.md      the criteria, when given
```

A case is a plain directory: copy it to where you tune the briefs, and edit its
`criteria.md` as you like. Only a run that recorded its input at its start can
be captured, and a name a case already has is refused. A capture that fails,
such as for a commit GitHub no longer has, writes nothing.
