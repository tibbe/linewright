End your session with a final message that is a single JSON object and nothing else:

```
{
  "outcome": "ready-for-agent" | "needs-info" | "ready-for-human",
  "category": {{category_choices}},
  "comment": "<the Markdown comment to post on the issue>",
  "ask": ["<for needs-info: the login of each person the question is for>"]
}
```

`ask` is for `needs-info` only. Leave it out to ask the reporter, @{{author}}.
