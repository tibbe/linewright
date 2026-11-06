End your session with a final message that is a single JSON object and nothing else, one of:

- `done`, with the pull request to open: `{ "outcome": "done", "pr_title": "<the pull request title, per the linewright:pr skill>", "pr_body": "<the pull request body, per the linewright:pr skill, without a closing reference to the ticket: Linewright adds it>", "spec_gaps": [SPEC_GAP], "declined_findings": [DECLINED] }`
- `blocked`, on a spec gap only the maintainer can answer: `{ "outcome": "blocked", "question": "<the question the maintainer must answer>", "spec_gaps": [SPEC_GAP] }`
- `gave-up`, when the work cannot be done: `{ "outcome": "gave-up", "reason": "<why>" }`

where

- `SPEC_GAP` is `{ "title": "<issue title>", "body": "<the spec change needed, in Markdown>" }`, and `spec_gaps` lists the gaps that did not block the ticket (often none).
- `DECLINED` is `{ "finding": "<the review finding, as the review put it>", "evidence": "<the quote that shows it is wrong on the facts>" }`, and `declined_findings` lists the review findings you did not apply (often none).
