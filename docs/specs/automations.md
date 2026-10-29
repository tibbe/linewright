# Automations

Automations are for work the pipeline doesn't start on its own: work the
maintainer describes in prose for an agent to do when a trigger fires. They
live in the repository, so adding or changing one is a reviewed change, and
they stay outside the pipeline, feeding it the way a person would, by filing
issues.

For example, an automation that runs every Monday morning looks at the bugs
fixed in the past week and files an issue for each that lacks a regression
test. The pipeline then works those issues like any other.

## User Stories

1. As an operator, I want an agent to run on a schedule with instructions I
   write, so that recurring work happens without me.
2. As an operator, I want an automation able to hand work to the pipeline, so
   that it is built and reviewed like any other work.
3. As an operator, I want automations defined in the repository, so that adding
   one is a reviewed change, versioned with the code.
4. As an operator, I want to run an automation by hand, so that I can try its
   instructions without waiting for its schedule.

## The automation file

An automation is a Markdown file in `.linewright/automations/` on the default
branch: its name is the file's name, its YAML front matter says when it runs,
and its body is the agent's instructions.

```markdown
---
on:
  schedule: "0 6 * * 1"
session-timeout: 60m
max-issues: 3
---
Look at the bugs fixed in the past week ...
```

| key           | meaning                                 | default  |
| ------------- | --------------------------------------- | -------- |
| `on.schedule` | when it runs: a cron expression, in UTC | required |
| `max-issues`  | the most issues one run files           | 3        |

- **Session settings.** The front matter also takes the session settings the
  repository's file takes
  ([The repository's file](operation.md#the-repositorys-file)), as a layer
  above every whole-pipeline layer, as a stage's table is
  ([Stage settings](operation.md#stage-settings)).
- **A file that does not parse.** Every file in `.linewright/automations/`,
  hidden files aside, is an automation file; one that does not parse is
  logged and skipped, and the other automations run.
- **`status`.** It shows each automation with its last and next run, and each
  file that does not parse.
- **The trigger.** `on` is a schedule, the only kind for now.

## Scheduling

An automation is due when a tick of its schedule has passed since its last
scheduled run.

- **Missed ticks.** Ticks missed while Linewright was down, or while a run of
  the automation was in flight, collapse into one run.
- **A new automation.** An automation seen for the first time counts as having
  had a scheduled run then, so adding one does not start it at once.
- **The last scheduled run.** Linewright records each automation's last
  scheduled run under the work directory.
- **By hand.** `lw automation run NAME` runs one automation now and exits, and
  leaves its schedule as it was. A run by hand is marked as one wherever the
  run is shown.
- **The last run.** An automation's last run is its newest, whether by its
  schedule or by hand.

## Runs

A run's sessions each work in a fresh clone of the default branch, and the run
counts against the ceiling on simultaneous runs.

- **Instructions.** The session is given the automation's instructions and
  nothing of earlier runs: an automation that looks back says how far in its
  instructions.
- **Filing issues.** A run may hand the pipeline work by filing issues, each
  saying which automation filed it:
  - `ready-for-agent`, for work an agent can do.
  - `ready-for-human`, when a person must decide or help.
- **Beyond `max-issues`.** Issues beyond `max-issues` are dropped and logged.

## The session

An automation's session is sent `automation.md`, with these placeholders
filled besides those of every prompt ([Contract](runs.md#contract)).

| placeholder        | value                          |
| ------------------ | ------------------------------ |
| `{{name}}`         | the automation's name          |
| `{{instructions}}` | its instructions               |
| `{{max_issues}}`   | its `max-issues`               |

The session ends with a result that matches
[`results/automation.schema.json`](results/automation.schema.json).

## Out of scope

- Other triggers, such as a merged pull request touching certain paths.
- Automations that open pull requests; they hand code changes to the pipeline
  as issues.
