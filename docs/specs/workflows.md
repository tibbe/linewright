# Workflows

Workflows are for work the pipeline doesn't start on its own: work the
maintainer describes in prose for an agent to do when a trigger fires. They
live in the repository, so adding or changing one is a reviewed change, and
they stay outside the pipeline, feeding it the way a person would, by filing
issues.

For example, a workflow that runs every Monday morning looks at the bugs
fixed in the past week and files an issue for each that lacks a regression
test. The pipeline then works those issues like any other.

## User Stories

1. As an operator, I want an agent to run on a schedule with instructions I
   write, so that recurring work happens without me.
2. As an operator, I want a workflow able to hand work to the pipeline, so that
   it is built and reviewed like any other work.
3. As an operator, I want workflows defined in the repository, so that adding
   one is a reviewed change, versioned with the code.
4. As an operator, I want to run a workflow by hand, so that I can try its
   instructions without waiting for its schedule.

## Decisions

- A **workflow** is a Markdown file in `.linewright/workflows/` on the
  default branch. Its name is the file's name, its YAML front matter says
  when it runs, and its body is the agent's instructions:

  ```markdown
  ---
  on:
    schedule: "0 6 * * 1"
  timeout: 60m
  max-issues: 3
  ---
  Look at the bugs fixed in the past week ...
  ```

  | key | meaning | default |
  |---|---|---|
  | `on.schedule` | when it runs: a cron expression, in UTC | required |
  | `timeout` | the session timeout of its runs | `session-timeout` |
  | `max-issues` | the most issues one run files | 3 |

  A file that does not parse is logged and skipped; the other workflows run.
- `status` shows each workflow with its last and next run, and each file
  that does not parse.
- The **trigger**, `on`, is a schedule, the only kind for now.
- A workflow is due when a tick of its schedule has passed since its last
  run. Ticks missed while Linewright was down, or while a run of the
  workflow was in flight, collapse into one run. A workflow seen for the
  first time counts as run then, so adding one does not start it at once.
  Linewright records each workflow's last run under the work directory.
- `lw workflow run NAME` runs one workflow now and exits, and
  leaves its schedule as it was.
- A run is one agent session in a fresh clone of the default branch, counted
  against the ceiling on simultaneous runs. The agent does what its
  instructions say. Its brief carries nothing of earlier runs: a workflow
  that looks back says how far in its instructions.
- A run may hand the pipeline work: issues it returns in its result, which
  the manager files, each opening with the marker line
  `> *Linewright, workflow <name>*`:
  - `ready-for-agent`, for work an agent can do: what to build and its
    acceptance criteria, like a ticket.
  - `ready-for-human`, when a person must decide or help.

  The agent searches the existing issues first and does not file one that
  is already open. Issues beyond `max-issues` are dropped and logged.
  Workflow issues skip triage, like tickets: triage passes over issues
  Linewright authored.
- A run that fails is logged, like any failure of Linewright's own.

## Out of scope

- Other triggers, such as a merged pull request touching certain paths.
- Workflows that open pull requests; they hand code changes to the pipeline
  as issues.
