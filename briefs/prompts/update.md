# Update

Bring a pull request's branch up to date with the default branch. You may read
GitHub but must not write to it or push: Linewright pushes your commits and
posts what your result says.

Resolve the conflicts keeping both sides' intent. Run the repository's checks
and fix what the update broke, then finish the update as your task says. Don't
abort it.

---

# Your task

- Default branch: `{{default_branch}}`
- Spec: the tracked files matching `{{spec_glob}}`
- Scratch files: in `$TMPDIR`, this session's own directory, never under a fixed path such as `/tmp`.
- Branch: `{{branch}}`, the head of pull request {{pr}} "{{pr_title}}".{{unfinished}}

{{conflict}}

### The pull request's description

{{description}}

{{ticket}}

---

# Result

End your session with a final message that is a single JSON object and nothing else:

```
One of:

{ "outcome": "done", "comment": "<Markdown summary of what you changed, for the pull request>", "pr_body": "<only when your change made the description out of date: the whole body rewritten, per the linewright:pr skill>", "replies": [REPLY], "spec_gaps": [SPEC_GAP] }
{ "outcome": "blocked", "question": "<the question the maintainer must answer>", "spec_gaps": [SPEC_GAP] }
{ "outcome": "gave-up", "reason": "<why the work cannot be done>" }

where REPLY is { "thread": "<thread id>", "body": "<your reply in that thread>", "resolve": <true to resolve it> }
and SPEC_GAP is { "title": "<issue title>", "body": "<the spec change needed, in Markdown>" }.
Reply in every thread with new feedback. Resolve only threads a bot opened: people resolve their own.
Leave `pr_body` out when the description still fits.
```
