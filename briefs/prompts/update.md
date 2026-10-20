# Your task

Bring the pull request's branch below up to date with the default branch. You may read GitHub but must not write to it or push: Linewright pushes your commits and posts what your result says.

- Default branch: `{{default_branch}}`
- Spec: the tracked files matching `{{spec_glob}}`
- Scratch files: in `$TMPDIR`, this session's own directory, never under a fixed path such as `/tmp`.
- Branch: `{{branch}}`, the head of pull request {{pr}} "{{pr_title}}".{{unfinished}}

{{conflict}}

### The pull request's description

{{description}}

{{ticket}}

## Steps

### 1. Resolve the conflicts

Resolve them keeping both sides' intent.

### 2. Run the checks

Run the repository's full checks (tests, linters, formatters, whatever its CI or docs say it runs) and fix what the update broke.

### 3. Finish the update

Finish it as your task says; don't abort it.

### 4. Output

End your session with a final message that is a single JSON object and nothing else, one of:

- `done`, with what you changed: `{ "outcome": "done", "comment": "<Markdown summary of what you changed, for the pull request>", "pr_body": "<the whole description rewritten, per the linewright:pr skill>", "replies": [REPLY], "spec_gaps": [SPEC_GAP] }`. Leave `pr_body` out unless your change made the description out of date.
- `blocked`, on a spec gap only the maintainer can answer: `{ "outcome": "blocked", "question": "<the question the maintainer must answer>", "spec_gaps": [SPEC_GAP] }`
- `gave-up`, when the work cannot be done: `{ "outcome": "gave-up", "reason": "<why>" }`

where

- `REPLY` is `{ "thread": "<thread id>", "body": "<your reply in that thread>", "resolve": <true to resolve it> }`. Reply in every thread with new feedback, and resolve only threads a bot opened: people resolve their own.
- `SPEC_GAP` is `{ "title": "<issue title>", "body": "<the spec change needed, in Markdown>" }`, and `spec_gaps` lists the gaps that did not block the work (often none).

## Spec gaps

The spec is the maintainer's: change no file of it. A question the spec leaves open is a spec gap only when it is a product decision: a reasonable maintainer could want either answer, the answer changes what a user sees, and the spec's wording doesn't already settle it. Anything else is yours to decide, as the spec and the code support best.

A gap that blocks the work is the question of a `blocked` result. For a gap that doesn't, choose the reading the spec and the code support best, carry on, and list it in `spec_gaps`.
