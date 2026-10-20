# Briefs

What Linewright gives its agents, read at startup from the `briefs` setting
(default: these, built into the binary):

- `plugin/`: the skills, a Claude Code plugin given to every session;
- `prompts/`: what Linewright sends a session, from its first message to the
  follow-ups, each with the result contract Linewright parses.

## `plugin/`

A skill per stage. Each is a fork of a skill from
[`mattpocock/skills`](https://github.com/mattpocock/skills) (commit
`f3fc5632`), under its MIT license (`plugin/LICENSE`), made non-interactive for
Linewright:

- each decision the original puts to a person is made by an agent or handed
  back as an outcome, and judging an agent's own work, as review does, falls
  to agents in fresh contexts;
- the agent may read the tracker, but writes to it only through the result
  (the JSON the session ends with), and what it needs from the tracker comes
  in the task Linewright gives it.

A skill is otherwise upstream's text, so that a diff against upstream shows
what the fork changed.

The plugin is named `linewright`, and every session is given a copy of it. A
prompt starts by invoking its stage's skill, such as `/linewright:implement`,
which calls `linewright:tdd`, `linewright:code-review` and `linewright:pr`. The
namespace keeps them apart from the harness's own skills and the repository's,
which sessions can use too.

| file | forked from | used by |
|---|---|---|
| `skills/triage/` | `triage` (with `AGENT-BRIEF.md`, `OUT-OF-SCOPE.md`) | triage |
| `skills/to-tickets/` | `to-tickets` | tickets |
| `skills/implement/` | `implement` | implementation, rework, CI fixes |
| `skills/tdd/` | `tdd` | called by `implement` |
| `skills/code-review/` | `code-review` | called by `implement` |
| `skills/pr/` | `pr` | the result's pull request title and body |

A skill says how to work, not what the result looks like: that is the prompt's.

## `prompts/`

Linewright's own. Each prompt is the session's first message as sent, with a
`{{name}}` placeholder for each value Linewright fills in, such as the ticket or
the default branch. The placeholders a prompt has are fixed: Linewright refuses
to start with a prompt that lacks one or has one it doesn't know.

| file | sent to |
|---|---|
| `triage.md` | triage |
| `tickets.md` | design |
| `implement.md` | implementation |
| `rework.md` | rework |
| `fix-ci.md` | CI fixes |
| `update.md` | an update's conflict, with no skill: the prompt is the brief |
| `automation.md` | automation runs |
| `parts/` | filled into the prompts above, such as a conflict's instructions |
| `follow-ups/` | a session sent back to work, such as for a result that doesn't parse |

A placeholder alone on its line that is filled with nothing takes the line, and
the blank line after it, with it. A prompt's line breaks are the session's, so
the prompts are not wrapped.
