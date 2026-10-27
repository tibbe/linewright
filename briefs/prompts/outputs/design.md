End your session with a final message that is a single JSON object and nothing else, one of:

- `designed`, with the design and, where the work is cut, the tickets: `{ "outcome": "designed", "design": "<the design, in Markdown, in the template below>", "tickets": [TICKET], "comment": "<Markdown: why the work is cut into these tickets>", "spec_gaps": [SPEC_GAP] }`. Leave `tickets` and `comment` out when the work is not cut.
- `nothing-to-build`, when the code already does what the {{kind}} asks: `{ "outcome": "nothing-to-build", "reason": "<Markdown: why>" }`
- `blocked`, on a spec gap only the maintainer can answer: `{ "outcome": "blocked", "question": "<the question the maintainer must answer>", "spec_gaps": [SPEC_GAP] }`
- `gave-up`, when the design cannot be made: `{ "outcome": "gave-up", "reason": "<why>" }`

where

- `TICKET` is `{ "id": "<a short id of your choosing, unique among the tickets>", "title": "<the ticket's title>", "body": "<the ticket's Markdown body>", "blocked_by": ["<ids of the tickets that must be merged first>"] }`
- `SPEC_GAP` is `{ "title": "<issue title>", "body": "<the spec change needed, in Markdown>" }`, and `spec_gaps` lists the gaps that did not block the design (often none).

The design, in this template:

```markdown
## Prior art

<the software-design skill's output, as it gave it, each of its headings one level down>

## Design

<what the code does today and where it falls short; the structure kept or changed, and why; the parts, what each is responsible for, where it lives, and the interfaces between them>
```

Linewright adds the tickets after it.
