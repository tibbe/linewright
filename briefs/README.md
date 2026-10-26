# Briefs

What Linewright gives its agents, built into the binary:

- `plugin/`: the skills, a Claude Code plugin given to every session;
- `prompts/`: what Linewright sends a session, from its first message to the
  follow-ups, and the result contracts Linewright parses.

The `briefs` setting names a directory laid out as this one, the operator's
layer over these: each file in it is taken in place of the one here, and every
file it lacks comes from here. Linewright logs at startup which files it takes
from the directory, and which files in it are outside this layout and so are
unused.

## `plugin/`

A skill per stage. Each is a fork of a published skill, from the source and
commit the table below gives. Those from
[`mattpocock/skills`](https://github.com/mattpocock/skills) (commit
`f3fc5632`) are under its MIT license (`plugin/LICENSE`), and are made
non-interactive for Linewright:

- each decision the original puts to a person is made by an agent or handed
  back as an outcome, and judging an agent's own work, as review does, falls
  to agents in fresh contexts;
- the agent may read the tracker, but writes to it only through the result
  (the JSON the session ends with), and what it needs from the tracker comes
  in the task Linewright gives it.

`software-design` is a fork of `prior-art` from
[`tibbe/skills`](https://github.com/tibbe/skills) (commit `e63caa77`), with
what concerns products removed, so that it designs code alone.

A skill is otherwise upstream's text, so that a diff against upstream shows
what the fork changed. The rules that are Linewright's own are the prompt's.

The plugin is named `linewright`, and every session is given a copy of it. A
prompt's task is the steps the session takes, in order, and the step a skill
covers has the session call it with the Skill tool: `linewright:implement`,
which calls `linewright:tdd`, `linewright:code-review` and `linewright:pr`; the
design prompt has the session call `linewright:software-design`, then
`linewright:to-tickets`. The namespace keeps them apart from the harness's own
skills and the repository's, which sessions can use too.

| file | forked from | used by |
|---|---|---|
| `skills/triage/` | `mattpocock/skills`: `triage` (with `AGENT-BRIEF.md`, `OUT-OF-SCOPE.md`) | triage |
| `skills/software-design/` | `tibbe/skills` (`e63caa77`): `prior-art` | design |
| `skills/to-tickets/` | `mattpocock/skills`: `to-tickets` | design |
| `skills/implement/` | `mattpocock/skills`: `implement` | implementation |
| `skills/tdd/` | `mattpocock/skills`: `tdd` | called by `implement` |
| `skills/code-review/` | `mattpocock/skills`: `code-review` | called by `implement` |
| `skills/pr/` | `mattpocock/skills`: `pr` | the result's pull request title and body |

A skill says how to work, not what the result looks like: that is the prompt's.

## `prompts/`

Linewright's own. Each prompt is the session's first message as sent, with a
`{{name}}` placeholder for each value Linewright fills in, such as the ticket or
the default branch. The placeholders a prompt has are fixed: Linewright refuses
to start with a prompt that lacks one or has one it doesn't know.

Each first-message prompt's result contract, the JSON its session ends with, is
a part of its own in `outputs/`, filled into the prompt's `{{output}}`. A prompt
written over keeps `{{output}}`, and so keeps the contract Linewright reads.

| file | sent to |
|---|---|
| `triage.md` | triage |
| `design.md` | design |
| `implement.md` | implementation |
| `rework.md` | rework, with no skill: the prompt is the brief |
| `fix-ci.md` | CI fixes, likewise |
| `update.md` | an update's conflict, likewise |
| `automation.md` | automation runs |
| `outputs/` | each prompt's result contract, filled into its `{{output}}`, such as `outputs/triage.md` into `triage.md` |
| `parts/` | filled into the prompts above, such as a conflict's instructions |
| `follow-ups/` | a session sent back to work, such as for a result that doesn't parse |

A placeholder alone on its line that is filled with nothing takes the line, and
the blank line after it, with it. A prompt's line breaks are the session's, so
the prompts are not wrapped.

Which runs read each file is declared beside the templates in
[`src/prompts.rs`](../src/prompts.rs) (`readers`); every run reads the plugin.
