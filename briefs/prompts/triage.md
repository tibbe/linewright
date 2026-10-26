# Your task

Triage the issue below.

{{facts}}
- Categories to choose from: {{categories}}

{{issue}}

## Steps

### 1. Triage the issue

Call the Skill tool with `linewright:triage` and work as it says.

### 2. Output

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
