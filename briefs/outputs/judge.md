End your session with a final message that is a single JSON object and nothing else, one of:

- `judged`: `{ "outcome": "judged", "correctness": { "reasoning": "<Markdown: for each criterion, whether the output meets it and what shows that>", "verdict": "pass" | "fail" }, "leak": { "reasoning": "<Markdown: where you looked and what you found>", "found": true | false, "calls": ["<a tool call that shows the leak, as the transcript has it>"] } }`. With no leak, leave `calls` out or empty.
- `gave-up`, when the output cannot be judged: `{ "outcome": "gave-up", "reason": "<why>" }`

Write each `reasoning` before you decide what follows it.
