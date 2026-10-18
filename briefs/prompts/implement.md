/linewright:implement

---

# Your task

- Default branch: `{{default_branch}}`
- Spec: the tracked files matching `{{spec_glob}}`
- Scratch files: in `$TMPDIR`, this session's own directory, never under a fixed path such as `/tmp`.
- Branch: `{{branch}}`, {{holding}} The fixed point for review is `origin/{{default_branch}}`.

{{ticket}}

---

# Result

End your session with a final message that is a single JSON object and nothing else:

```
One of:

{ "outcome": "done", "pr_title": "<the pull request title, per the linewright:pr skill>", "pr_body": "<the pull request body, per the linewright:pr skill, without a closing reference to the ticket: Linewright adds it>", "spec_gaps": [SPEC_GAP], "declined_findings": [DECLINED] }
{ "outcome": "blocked", "question": "<the question the maintainer must answer>", "spec_gaps": [SPEC_GAP] }
{ "outcome": "gave-up", "reason": "<why the work cannot be done>" }

where SPEC_GAP is { "title": "<issue title>", "body": "<the spec change needed, in Markdown>" }
and DECLINED is { "finding": "<the review finding, as the review put it>", "evidence": "<the quote that shows it is wrong on the facts>" };
`spec_gaps` lists the gaps that did not block the ticket (often none), and `declined_findings` the review findings you did not apply (often none).
```
