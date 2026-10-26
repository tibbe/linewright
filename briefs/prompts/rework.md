# Your task

Address the feedback below on the pull request. You may read GitHub but must not write to it or push: Linewright pushes your commits, replies in the threads and posts what your result says.

{{facts}}
- Branch: `{{branch}}`, the head of pull request {{pr}} "{{pr_title}}".{{unfinished}}

### Feedback to address

{{feedback}}

### The pull request's conversation so far

{{conversation}}

### The pull request's description

{{description}}

{{ticket}}

## Steps

{{conflict}}

### 1. Read the feedback

Read the feedback, the conversation it is part of, the ticket and the parts of the spec the feedback touches. Address every comment from a person; a comment from another bot at your judgment, and you may answer that it needs no change.

### 2. Make the changes

Make the changes the feedback asks for, with tests where a change needs them. Then run the repository's full checks (tests, linters, formatters, whatever its CI or docs say it runs) and fix what fails.

### 3. Commit

Commit your work to the current branch, with messages that say why. Leave no uncommitted changes and no rebase or merge in progress.

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
