# Developing Linewright

## Tuning the briefs

Each stage's agent works from a brief in [`briefs/`](../briefs/). To try a
change against a real case:

```sh
linewright capture owner/name#123 case/      # a spec change or an issue
linewright --briefs briefs/ replay case/ --out run-1/
```

`run-1/` holds the issues and pull requests the run produced (`issues/`,
`pulls/`, as JSON, Markdown and diffs) and every session's brief, result and
trace (`sessions/`).

## Testing

```sh
cargo test
```

Tests sit where [`CODING_STANDARDS.md`](../CODING_STANDARDS.md) puts them:
`tests/reconcile.rs` drives the decision function, the workflows' due rule
included, `tests/status.rs` the `status` report, `tests/workflow_file.rs` the
workflow-file parser, and
`tests/pipeline.rs`, `tests/workflow_run.rs`, `tests/workflow_schedule.rs`,
`tests/replay.rs` and `tests/config.rs` drive the system as a user would,
against a local store standing in for GitHub and a scripted agent runtime.
`tests/runtime.rs` drives the Claude Code runtime against a stand-in `claude`
program, for what a production session gets that replay can't show.
