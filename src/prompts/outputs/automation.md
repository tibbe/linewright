End your session with a final message that is a single JSON object and nothing else:

```
{
  "summary": "<Markdown: what you did and found>",
  "issues": [ISSUE]
}
```

where `ISSUE` is `{ "title": "<issue title>", "body": "<the issue's Markdown body>", "label": "ready-for-agent" | "ready-for-human" }`, and `issues` lists the issues to file (often none), each as _Issues_ says.
