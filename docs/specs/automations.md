# Automations

Automations are for work the pipeline doesn't start on its own: work the
maintainer describes in prose for an agent to do when a trigger fires. They
live in the repository, so adding or changing one is a reviewed change, and
they stay outside the pipeline, feeding it the way a person would, by filing
issues.

For example, an automation that runs every Monday morning looks at the bugs
fixed in the past week and files an issue for each that lacks a regression
test. The pipeline then works those issues like any other. Another runs
when a workflow run fails on the default branch, and files an issue to fix
what failed.

## User Stories

1. As an operator, I want an agent to run on a schedule with instructions I
   write, so that recurring work happens without me.
2. As an operator, I want an automation able to hand work to the pipeline, so
   that it is built and reviewed like any other work.
3. As an operator, I want automations defined in the repository, so that adding
   one is a reviewed change, versioned with the code.
4. As an operator, I want to run an automation by hand, so that I can try its
   instructions without waiting for its trigger.
5. As a maintainer, I want to see each automation's last and next run, and
   each automation file that does not parse, so that I know my automations
   run.
6. As an operator, I want an agent to run when a workflow run completes, so
   that the factory repairs a failure on the default branch without me.

## The automation file

An automation is a Markdown file anywhere under `.linewright/automations/` on
the default branch: its name is its path there, such as `team/weekly.md`, its
YAML front matter says when it runs, and its body is the agent's instructions.

```markdown
---
on:
  schedule: "0 6 * * 1"
session-timeout: 60m
max-issues: 3
---
Look at the bugs fixed in the past week ...
```

| key          | meaning                       | default  |
| ------------ | ----------------------------- | -------- |
| `on`         | its trigger                   | required |
| `max-issues` | the most issues one run files | 3        |

| trigger           | it runs                                                         |
| ----------------- | --------------------------------------------------------------- |
| `on.schedule`     | on a schedule: a cron expression, in UTC                        |
| `on.workflow_run` | when a workflow run completes ([Workflow runs](#workflow-runs)) |

- **Session settings.** The front matter also takes the session settings the
  repository's file takes
  ([The repository's file](operation.md#the-repositorys-file)), as a layer
  above every whole-pipeline layer, as a stage's table is
  ([Stage settings](operation.md#stage-settings)).
- **A file that does not parse.** Every file in `.linewright/automations/`,
  hidden files aside, is an automation file; one that does not parse is
  logged and skipped, and the other automations run.
- **`status`.** It shows each automation with its trigger and its last run,
  and each file that does not parse. For an automation on a schedule, it
  also shows the next run.
- **The trigger.** `on` holds exactly one trigger of the table. A file with
  any other `on` does not parse.

## Scheduling

An automation is due when a tick of its schedule has passed since its last
scheduled run.

- **Missed ticks.** Ticks missed while Linewright was down, or while a run of
  the automation was in flight, collapse into one run.
- **A new automation.** An automation counts as having had a scheduled run
  when Linewright first sees it, or first sees it with a schedule, so adding
  one does not start it at once.
- **The last scheduled run.** Linewright records each automation's last
  scheduled run in its database ([Storage](architecture.md#storage)).

## Workflow runs

An automation with `on.workflow_run` runs once for each attempt of a workflow
run that completes and matches all the trigger's filters.

```markdown
---
on:
  workflow_run:
    workflows: [CI]
    branches: [main]
    conclusions: [failure]
---
A workflow run failed on the default branch ...
```

| filter        | matches                                 | values |
| ------------- | --------------------------------------- | ------ |
| `workflows`   | the name of the workflow run's workflow | any    |
| `branches`    | the workflow run's branch               | any    |
| `conclusions` | the conclusion GitHub gives the attempt | `success`, `failure`, `cancelled`, `skipped`, `timed_out`, `action_required`, `neutral`, `stale`, `startup_failure` |

- **Filters.** A filter is a list, and matches what is equal to one of its
  values. A filter left out matches everything.
- **Filters that do not parse.** A file does not parse when a filter:
  - is empty;
  - is not in the table;
  - holds a conclusion that is not in the table.
- **The branch.** A workflow run's branch is the head branch GitHub gives it.
  A workflow run for a fork's commit has none.
- **The time limit.** An attempt runs the automation only when Linewright
  sees it within `ci-timeout` after GitHub created its workflow run.
- **Earlier attempts.** An attempt that completed before Linewright first saw
  the automation's present `on` does not run it. One that completed later
  while Linewright was down does.
- **In order.** The runs that an automation's trigger starts go one at a
  time, in the order in which the attempts completed.
- **A cancelled run.** An attempt whose run is cancelled
  ([Ending](runs.md#ending)) runs the automation again.
- **What has run it.** Linewright records the attempts that have run each
  automation in its database ([Storage](architecture.md#storage)).

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
- **By hand.** `lw automation run NAME` runs one automation now and exits. It
  changes neither the automation's schedule nor which attempts have run it.
  A run by hand is marked as one wherever the run is shown.
- **The last run.** An automation's last run is its newest, whether by its
  trigger or by hand.

## The session

An automation's session is sent `automation.md`, with these placeholders
filled besides those of every prompt ([Contract](runs.md#contract)).

| placeholder        | value                          |
| ------------------ | ------------------------------ |
| `{{name}}`         | the automation's name          |
| `{{instructions}}` | its instructions               |
| `{{max_issues}}`   | its `max-issues`               |
| `{{trigger}}`      | the workflow run that started it (`trigger-workflow-run.md`), or nothing |

- **`{{trigger}}`.** It gives the workflow run's workflow, attempt,
  conclusion, commit and link, and its branch where it has one.

The session ends with a result that matches
[`briefs/results/automation.schema.json`](../../briefs/results/automation.schema.json).

## Screens

| Screen      | Reached from                                                                                                                                         | Purpose                                         |
| ----------- | ---------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------- |
| Automations | the navigation ([Shell](shell.md)), an automation's name where a screen names its run, and the notice of an automation file that does not parse | see each automation, its last run and its next |

### Automations

- **Address.** `/automations`.
- **Layout.** Each automation file that does not parse, then a row per
  automation.
- **A file that does not parse** shows as failed
  ([Status](ui_patterns.md#status)), and reads "`FILE` does not parse:
  ERROR", where FILE is its name and ERROR is what the parser gives. It is
  drawn as a notice is ([Shell](shell.md)), without a link.

| Column     | Holds                                                                                                  |
| ---------- | ------------------------------------------------------------------------------------------------------ |
| Automation | the automation's name                                                                                  |
| Trigger    | when it runs, in words                                                                                 |
| Last run   | its last run, with its outcome, linking to its Run; or, before its first, when Linewright first saw it |
| Next run   | when it runs next; nothing for an automation not on a schedule                                         |

## Out of scope

- Other triggers, such as a merged pull request touching certain paths, a
  check suite or a commit status.
- More than one trigger in an automation.
- Filters that match by pattern, such as a glob of branch names.
- Automations that open pull requests; they hand code changes to the pipeline
  as issues.
