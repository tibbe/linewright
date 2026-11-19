# Tuning the briefs

Each stage's agent works from a brief in [`briefs/`](../briefs/): the skills in
`plugin/`, the prompts in `prompts/`, and Linewright's wording of what it fills
into the prompts' placeholders in `parts/` and `outputs/`, as its README
describes. A `--briefs` directory is laid out as `briefs/` is, such as
`prompts/implement.md`, `parts/result-no-json.md` and
`plugin/skills/tdd/SKILL.md`, and holds only the files it changes: the rest are
Linewright's own. A file under `results/`, or any other file that is not one of
Linewright's own under those four directories, is an error, so a change to a
result schema is tried by editing `briefs/results/` and rebuilding.

A change to a file's placeholders, which Linewright refuses unless the code
changes with it, is a change to the spec (`docs/specs/runs.md`, "Contract").

## Capturing a task

A triage, design or implement run that went badly is worth keeping as a task,
to try a change to its stage's brief against (`docs/specs/benchmarks.md`). Find
the run's ID with `lw runs N`, such as `43/implement-2`, and capture it into a
benchmark of its stage, under a name of lowercase letters, digits and hyphens
starting with a letter or a digit, with what a good result would have been:

```sh
lw benchmark add implements implement
lw capture 43/implement-2 43-implement-2 --benchmark implements --criteria good-result.md
```

It works whether `serve` is running or not, with the token `serve` uses: the
commits are fetched from GitHub. The task is written to `tasks/43-implement-2/`
under the work directory:

```text
task.json        the stage, the item, the start, the run and the API version
items.json       the items the run was given, as GitHub's REST API gave them
properties.json  the repository's properties, as GitHub's API gave them
repository.git/  a bare repository of the branch heads it was given
criteria.md      the criteria, which a task needs
```

A task is a plain directory: copy it to where you tune the briefs, and edit its
`criteria.md` as you like. Only a run that recorded its whole input at its start
can be captured: a run recorded before Linewright recorded the repository's
properties is refused. A name a task already has is refused too, and so is a
benchmark of another stage. A capture that fails, such as for a commit GitHub no
longer has, writes nothing.

`lw benchmarks` lists the benchmarks, `lw benchmark show NAME` shows a
benchmark's tasks with their criteria and its benchmark runs, and
`lw benchmark remove-task NAME TASK` removes a task, its directory with it. A
task captured before benchmarks is in none. The web UI does the same from its
benchmarks page, at `/benchmarks`: it lists the benchmarks and makes one, and
each benchmark's page shows its tasks, captures a run of its stage into it and
removes a task.
A benchmark run compares configurations on a benchmark's tasks
([Benchmark runs](running.md#benchmark-runs)).
