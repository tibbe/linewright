/linewright:triage

---

# Your task

- Default branch: `{{default_branch}}`
- Spec: the tracked files matching `{{spec_glob}}`
- Scratch files: in `$TMPDIR`, this session's own directory, never under a fixed path such as `/tmp`.
- Categories to choose from: {{categories}}

{{issue}}

---

# Result

End your session with a final message that is a single JSON object and nothing else:

```
{
  "outcome": "ready-for-agent" | "needs-info" | "ready-for-human",
  "category": {{category_choices}},
  "comment": "<the Markdown comment to post on the issue>",
  "ask": ["<for needs-info: the login of each person the question is for>"]
}

`ask` is for `needs-info` only. Leave it out to ask the reporter, @{{author}}.
```
