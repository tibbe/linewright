# Briefs

What Linewright gives its agents, built into the binary:

- `plugin/`: the skills, a Claude Code plugin given to every session;
- `prompts/`: what Linewright sends a session, from its first message to the
  follow-ups;
- `parts/` and `outputs/`: Linewright's own wording of what it fills into the
  prompts' placeholders, such as a result contract or why a result was sent
  back;
- `results/`: each session's result schema, which Linewright checks the result
  against.

Every text Linewright sends an agent is a file here. An operator or a
repository can replace any of them but the result schemas, as
[Briefs](../docs/specs/operation.md#briefs) says.

## `plugin/`

There is a skill per stage, from the source the table below gives. Those from
[`mattpocock/skills`](https://github.com/mattpocock/skills) are under its MIT
license (`plugin/LICENSE`). Linewright makes them non-interactive:

- an agent makes each decision the original puts to a person, or hands it back
  as an outcome, and agents in fresh contexts judge an agent's own work, as
  review does;
- the agent writes to the tracker only through the result, the JSON the session
  ends with, and the task Linewright gives it carries what it needs from the
  tracker.

The plugin's name is `linewright`, and every session gets a copy of it. A
prompt's task is the steps the session takes, in order, and the step a skill
covers has the session call it with the Skill tool. The namespace keeps them
apart from the harness's own skills and the repository's, which sessions can
use too.

| file | origin | used by |
|---|---|---|
| `skills/triage/` | `mattpocock/skills`: `triage` (with `AGENT-BRIEF.md`, `OUT-OF-SCOPE.md`) | triage |
| `skills/software-design/` | [`tibbe/skills`](https://github.com/tibbe/skills): `prior-art` | design |
| `skills/prototype/` | `mattpocock/skills`: `prototype`, kept to the claims a design choice turns on | called by `software-design` |
| `skills/to-tickets/` | `mattpocock/skills`: `to-tickets` | design |
| `skills/implement/` | `mattpocock/skills`: `implement` | implementation |
| `skills/tdd/` | `mattpocock/skills`: `tdd` | called by `implement` |
| `skills/code-review/` | `mattpocock/skills`: `code-review` | called by `implement` |
| `skills/pr/` | `mattpocock/skills`: `pr` | the result's pull request title and body |

## `prompts/`

Each prompt is the session's first message as
sent, with a `{{name}}` placeholder for each value Linewright fills in, such as
the ticket or the default branch. The placeholders a prompt has are fixed.
Linewright refuses to start with a prompt that lacks one or has one it doesn't
know.

| file | sent to |
|---|---|
| `triage.md` | triage |
| `design.md` | design |
| `implement.md` | implementation |
| `rework.md` | rework, with no skill: the prompt is the brief |
| `fix-ci.md` | CI fixes, likewise |
| `update.md` | an update's conflict, likewise |
| `automation.md` | automation runs |
| `judge.md` | the judge of a replay |
| `follow-ups/` | a session sent back to work, such as for a result that doesn't parse |

When a placeholder alone on its line is filled with nothing, its line and the
blank line after it go too. A prompt's line breaks reach the session as they
are, so the prompts are not wrapped.

## `parts/` and `outputs/`

What Linewright fills into a prompt's placeholders: `outputs/` holds each
stage's result contract, which fills `{{output}}`, and `parts/` the rest, such
as the attachments contract appended to every `{{output}}` and the parts of a
follow-up, among them each reason Linewright gives for sending a result back.
A layer may hold them too; a result contract it rewords still has to describe
the result that the schema in `results/` accepts. Like the prompts, they are
not wrapped.

[`src/prompts.rs`](../src/prompts.rs) declares, beside the templates
(`readers`), which runs read each file. Every run gets the plugin.
