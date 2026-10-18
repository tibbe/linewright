/linewright:implement

---

# Your task

- Default branch: `{{default_branch}}`
- Spec: the tracked files matching `{{spec_glob}}`
- Scratch files: in `$TMPDIR`, this session's own directory, never under a fixed path such as `/tmp`.
- Branch: `{{branch}}`, the head of pull request {{pr}} "{{pr_title}}". The fixed point for review is `origin/{{default_branch}}`.{{unfinished}}

{{conflict}}

### Feedback to address

{{feedback}}

### The pull request's conversation so far

{{conversation}}

### The pull request's description

{{description}}

{{ticket}}

---

# Result

End your session with a final message that is a single JSON object and nothing else:

```
One of:

{ "outcome": "done", "comment": "<Markdown summary of what you changed, for the pull request>", "pr_body": "<only when your change made the description out of date: the whole body rewritten, per the linewright:pr skill>", "replies": [REPLY], "spec_gaps": [SPEC_GAP], "declined_findings": [DECLINED] }
{ "outcome": "blocked", "question": "<the question the maintainer must answer>", "spec_gaps": [SPEC_GAP] }
{ "outcome": "gave-up", "reason": "<why the work cannot be done>" }

where REPLY is { "thread": "<thread id>", "body": "<your reply in that thread>", "resolve": <true to resolve it> }
and SPEC_GAP is { "title": "<issue title>", "body": "<the spec change needed, in Markdown>" },
and DECLINED is { "finding": "<the review finding, as the review put it>", "evidence": "<the quote that shows it is wrong on the facts>" }.
`declined_findings` lists the review findings you did not apply (often none).
Reply in every thread with new feedback. Resolve only threads a bot opened: people resolve their own.
Leave `pr_body` out when the description still fits.
```
