# Briefs

A skill per stage and a brief for workflow runs, read at startup from the
`briefs` setting (default: these, built into the binary). Each skill is a fork
of a skill from [`mattpocock/skills`](https://github.com/mattpocock/skills)
(commit `f3fc5632`), under its MIT license (`LICENSE`), made non-interactive
for Linewright:

- each decision the original puts to a person is made by an agent or handed
  back as an outcome, and judging an agent's own work, as review does, falls
  to agents in fresh contexts;
- the agent may read the tracker, but writes to it only through the result
  (the JSON the session ends with), and what it needs from the tracker comes
  in the task Linewright gives it.

This directory is a Claude Code plugin, `linewright`, and every session is given
a copy of it. Each stage's session starts by invoking its skill, such as
`/linewright:implement`, which calls `linewright:tdd`, `linewright:code-review`
and `linewright:pr`. The namespace keeps them apart from the runtime's own
skills and the repository's, which sessions can use too.

| file | forked from | used by |
|---|---|---|
| `skills/triage/` | `triage` (with `AGENT-BRIEF.md`, `OUT-OF-SCOPE.md`) | triage |
| `skills/to-tickets/` | `to-tickets` | tickets |
| `skills/implement/` | `implement` | implementation, rework, CI fixes, conflicts |
| `skills/tdd/` | `tdd` | called by `implement` |
| `skills/code-review/` | `code-review` | called by `implement` |
| `skills/pr/` | `pr` | the result's pull request title and body |
| `workflow.md` | Linewright's own | workflow runs |

Linewright gives each stage's skill the task (the issue, pull request or spec
diff) and the result contract as its input, and appends them to the workflow
brief, so a skill or brief says how to work, not what the result looks like.
