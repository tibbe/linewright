# Your task

Run one of the maintainer's automations, the one below: do what its instructions say. You work in a fresh clone of the default branch, and nobody is there to answer questions, so make the decisions the instructions leave to you. You may read GitHub but must not write to it: no issues, comments, labels, pull requests or pushes. The issues you want filed go in your result, and Linewright files them.

- Default branch: `{{default_branch}}`
- Spec: the tracked files matching `{{spec_glob}}`
- Scratch files: in `$TMPDIR`, this session's own directory, never under a fixed path such as `/tmp`.
- Issues: at most {{max_issues}}.

### The automation `{{name}}`

{{instructions}}

## Steps

### 1. Read the instructions

Read `GLOSSARY.md` if present, and the spec when the instructions touch what the software should do.

### 2. Do the work

Do the work the instructions describe. You may read anything your session reaches: the repository, GitHub, the web. Build and run the code when that is how to find out.

### 3. Output

End your session with a final message that is a single JSON object and nothing else:

```
{
  "summary": "<Markdown: what you did and found>",
  "issues": [ISSUE]
}
```

where `ISSUE` is `{ "title": "<issue title>", "body": "<the issue's Markdown body>", "label": "ready-for-agent" | "ready-for-human" }`, and `issues` lists the issues to file (often none), each as _Issues_ says.

## Earlier runs

You know nothing of this automation's earlier runs, and nothing you leave in the clone survives the session. When the instructions look back, they say how far.

## Issues

Each issue is work for the pipeline or for the maintainer:

- **`ready-for-agent`**: work an agent can do without a person deciding anything. Write it like a ticket: what to build and its acceptance criteria. It goes straight to an implementer, without triage, so it must stand on its own.
- **`ready-for-human`**: a person must decide or help. Say what, and recommend.

Search the existing issues first, and don't return one that is already open. Keep to the number of issues your task allows; when you find more, hand back the most important ones. Linewright opens each issue's body with a line naming the automation, so leave that out.
