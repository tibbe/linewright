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
