# Briefs

What Linewright gives its agents, built into the binary:

- `plugin/`: the skills, a Claude Code plugin given to every session;
- `prompts/`: what Linewright sends a session, from its first message to the
  follow-ups, and the result contracts Linewright parses.

The skills are always these. The `prompts` setting names a directory laid out
as `prompts/` is, the operator's layer over it. Linewright takes each file in
it in place of the one here, and every file it lacks comes from here. At
startup it logs which files it takes from the directory. A file in it that is
not one of the prompts here, such as a misspelt `implment.md`, stops Linewright
at startup with an error naming it. Linewright skips hidden files, whose name
or directory's name starts with `.`.

## `plugin/`

There is a skill per stage, from the source the table below gives. Those from
[`mattpocock/skills`](https://github.com/mattpocock/skills) are under its MIT
license (`plugin/LICENSE`). Linewright makes them non-interactive:

- an agent makes each decision the original puts to a person, or hands it back
  as an outcome, and agents in fresh contexts judge an agent's own work, as
  review does;
- the agent may read the tracker but writes to it only through the result, the
  JSON the session ends with, and the task Linewright gives it carries what it
  needs from the tracker.

The plugin's name is `linewright`, and every session gets a copy of it. A
prompt's task is the steps the session takes, in order, and the step a skill
covers has the session call it with the Skill tool. The implement prompt calls
`linewright:implement`, which calls `linewright:tdd`, `linewright:code-review`
and `linewright:pr`. The design prompt calls `linewright:software-design`, then
`linewright:to-tickets`. The namespace keeps them apart from the harness's own
skills and the repository's, which sessions can use too.

| file | origin | used by |
|---|---|---|
| `skills/triage/` | `mattpocock/skills`: `triage` (with `AGENT-BRIEF.md`, `OUT-OF-SCOPE.md`) | triage |
| `skills/software-design/` | [`tibbe/skills`](https://github.com/tibbe/skills): `prior-art` | design |
| `skills/to-tickets/` | `mattpocock/skills`: `to-tickets` | design |
| `skills/implement/` | `mattpocock/skills`: `implement` | implementation |
| `skills/tdd/` | `mattpocock/skills`: `tdd` | called by `implement` |
| `skills/code-review/` | `mattpocock/skills`: `code-review` | called by `implement` |
| `skills/pr/` | `mattpocock/skills`: `pr` | the result's pull request title and body |

## `prompts/`

These are Linewright's own. Each prompt is the session's first message as
sent, with a `{{name}}` placeholder for each value Linewright fills in, such as
the ticket or the default branch. The placeholders a prompt has are fixed.
Linewright refuses to start with a prompt that lacks one or has one it doesn't
know.

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

When a placeholder alone on its line is filled with nothing, its line and the
blank line after it go too. A prompt's line breaks reach the session as they
are, so the prompts are not wrapped.

[`src/prompts.rs`](../src/prompts.rs) declares, beside the templates
(`readers`), which runs read each file. Every run gets the plugin.
