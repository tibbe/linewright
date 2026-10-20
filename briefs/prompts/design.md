/linewright:software-design

---

# Your task

Design the code for the {{kind}} below, then cut the work as _Cutting_ says.

- Default branch: `{{default_branch}}`
- Spec: the tracked files matching `{{spec_glob}}`
- Scratch files: in `$TMPDIR`, this session's own directory, never under a fixed path such as `/tmp`.

{{item}}

{{filed_gaps}}

## Keeping or changing the structure

Keep the code's structure where it holds up. Change it where the change would add a special case to it, or where the structure makes the problem likely, so that the same kind of problem would come back.

A changed structure covers the part the change touches, judged against the whole spec rather than the {{kind}} alone.

## Cutting

{{cutting}}

## Spec gaps

The spec is the maintainer's: change no file of it. A question the spec leaves open is a spec gap only when it is a product decision: a reasonable maintainer could want either answer, the answer changes what a user sees, and the spec's wording doesn't already settle it. Anything else is yours to decide, as the spec and the code support best.

A gap that blocks the design is the question of a `blocked` result. For a gap that doesn't, choose the reading the spec and the code support best, carry on, and list it in `spec_gaps`.

---

# Result

End your session with a final message that is a single JSON object and nothing else:

```
One of:

{ "outcome": "designed", "design": "<the design, in Markdown: the parts, what each is responsible for, the interfaces between them, and the alternatives it was chosen against>", "tickets": [TICKET], "comment": "<Markdown: why the work is cut into these tickets>", "spec_gaps": [SPEC_GAP] }
{ "outcome": "nothing-to-build", "reason": "<Markdown: why the code already does what the {{kind}} asks>" }
{ "outcome": "blocked", "question": "<the question the maintainer must answer>", "spec_gaps": [SPEC_GAP] }
{ "outcome": "gave-up", "reason": "<why the design cannot be made>" }

where TICKET is { "id": "<a short id of your choosing, unique among the tickets>", "title": "<the ticket's title>", "body": "<the ticket's Markdown body>", "blocked_by": ["<ids of the tickets that must be merged first>"] }
and SPEC_GAP is { "title": "<issue title>", "body": "<the spec change needed, in Markdown>" };
`tickets` and `comment` are for work cut into tickets, and left out when it is not; `spec_gaps` lists the gaps that did not block the design (often none).
```
