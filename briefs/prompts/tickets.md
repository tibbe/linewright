/linewright:to-tickets

---

# Your task

- Default branch: `{{default_branch}}`
- Spec: the tracked files matching `{{spec_glob}}`
- Scratch files: in `$TMPDIR`, this session's own directory, never under a fixed path such as `/tmp`.

### The spec change: {{number}} {{title}}

Merged by its author @{{author}} into `{{default_branch}}`.

{{body}}

#### Spec diff

```diff
{{spec_diff}}
```

---

# Result

End your session with a final message that is a single JSON object and nothing else:

```
{
  "tickets": [
    {
      "id": "<a short id of your choosing, unique among the tickets>",
      "title": "<the ticket's title>",
      "body": "<the ticket's Markdown body>",
      "blocked_by": ["<ids of the tickets that must be merged first>"]
    }
  ],
  "comment": "<Markdown summary of the breakdown, or why no tickets are needed>"
}

`tickets` is empty when the code already does what the spec change asks.
```
