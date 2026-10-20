# Your task

Design the code for the {{kind}} below. You may read GitHub but must not write to it.

- Default branch: `{{default_branch}}`
- Spec: the tracked files matching `{{spec_glob}}`
- Scratch files: in `$TMPDIR`, this session's own directory, never under a fixed path such as `/tmp`.

{{item}}

{{filed_gaps}}

## Steps

### 1. Research prior art and choose a design

Read the {{kind}}, `GLOSSARY.md` if present, the spec and the code the {{kind}} touches. Then call the Skill tool with `linewright:software-design`, and find how the problem has been solved before and choose a design from it, as it says.

### 2. Keep or change the structure

Keep the code's structure where it holds up. Change it where the change would add a special case to it, or where the structure makes the problem likely, so that the same kind of problem would come back.

A changed structure covers the part the change touches, judged against the whole spec rather than the {{kind}} alone.

### 3. Cut the work

{{cutting}}

### 4. Output

End your session with a final message that is a single JSON object and nothing else, one of:

- `designed`, with the design and, where the work is cut, the tickets: `{ "outcome": "designed", "design": "<the design, in Markdown: the parts, what each is responsible for, the interfaces between them, and the alternatives it was chosen against>", "tickets": [TICKET], "comment": "<Markdown: why the work is cut into these tickets>", "spec_gaps": [SPEC_GAP] }`. Leave `tickets` and `comment` out when the work is not cut.
- `nothing-to-build`, when the code already does what the {{kind}} asks: `{ "outcome": "nothing-to-build", "reason": "<Markdown: why>" }`
- `blocked`, on a spec gap only the maintainer can answer: `{ "outcome": "blocked", "question": "<the question the maintainer must answer>", "spec_gaps": [SPEC_GAP] }`
- `gave-up`, when the design cannot be made: `{ "outcome": "gave-up", "reason": "<why>" }`

where

- `TICKET` is `{ "id": "<a short id of your choosing, unique among the tickets>", "title": "<the ticket's title>", "body": "<the ticket's Markdown body>", "blocked_by": ["<ids of the tickets that must be merged first>"] }`
- `SPEC_GAP` is `{ "title": "<issue title>", "body": "<the spec change needed, in Markdown>" }`, and `spec_gaps` lists the gaps that did not block the design (often none).

## Spec gaps

The spec is the maintainer's: change no file of it. A question the spec leaves open is a spec gap only when it is a product decision: a reasonable maintainer could want either answer, the answer changes what a user sees, and the spec's wording doesn't already settle it. Anything else is yours to decide, as the spec and the code support best.

A gap that blocks the design is the question of a `blocked` result. For a gap that doesn't, choose the reading the spec and the code support best, carry on, and list it in `spec_gaps`.
