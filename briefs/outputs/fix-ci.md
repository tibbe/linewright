End your session with a final message that is a single JSON object and nothing else, one of:

- `done`, with what you changed: `{ "outcome": "done", "comment": "<Markdown summary of what you changed, for the pull request>", "pr_body": "<the whole description rewritten, per the linewright:pr skill>", "replies": [REPLY], "spec_gaps": [SPEC_GAP] }`. Leave `pr_body` out unless your change made the description out of date.
- `blocked`, on a spec gap only the maintainer can answer: `{ "outcome": "blocked", "question": "<the question the maintainer must answer>", "spec_gaps": [SPEC_GAP] }`
- `gave-up`, when the work cannot be done: `{ "outcome": "gave-up", "reason": "<why>" }`

where

- `REPLY` is `{ "thread": "<thread id>", "body": "<your reply in that thread>", "resolve": <true to resolve it> }`. Reply in every thread with new feedback, and resolve only threads a bot opened: people resolve their own.
- `SPEC_GAP` is `{ "title": "<issue title>", "body": "<the spec change needed, in Markdown>" }`, and `spec_gaps` lists the gaps that did not block the work (often none).
