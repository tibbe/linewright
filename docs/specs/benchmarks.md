# Benchmarks

How well a stage works depends on its brief, its settings and its model. A
change to one of them can help some work and harm other work, and it can
change what the work costs. Benchmarks let a maintainer try a change on work
that the stage has already done, before the change reaches new work, and see
whether it helps and what it costs.

For example, a maintainer finds that the design of #812 cut the work badly.
They capture the run as a task, with what a good design would have done. They
then try a change to the brief of the design stage, and see whether it fixes
#812 without harm to their other design tasks, and what it costs.

## User Stories

1. As a maintainer, I want to capture a run of a stage from its screen, so
   that I can run that stage on the same input again.
2. As a maintainer, I want a task to hold only what its run was given and
   nothing from after it started, so that the stage run on it again has
   nothing that shows how the work turned out.
3. As a maintainer, I want a task to stay usable as Linewright changes, so that
   tasks captured long ago still measure the stage as it is today.
4. As a maintainer, I want to say what a good result is as I capture a task, so
   that results can be scored against it.
5. As an agent, I want to capture a task from the command line, so that I can
   build tasks without the UI.
6. As a maintainer, I want to run a task's stage again with a setting or its
   brief changed, so that I see what the change does to real work.
7. As a maintainer, I want Linewright to write nothing to GitHub in a trial,
   so that I can try a change without touching the repository.
8. As an agent, I want a trial's output as plain files, so that I can read
   it, score it and change the briefs.
9. As a maintainer, I want the `gh` of a trial's session kept from the task's
    repository on GitHub, so that the session cannot read how the work turned
    out.
10. As a maintainer, I want a scorer to say whether the output of a trial
    meets the criteria of the task, so that I know whether a change helped
    and do not read every output.
11. As a maintainer, I want the scorer to tell me when a session learned how
    the work turned out, so that I do not trust that trial.
12. As a maintainer, I want a trial to say what it cost, so that I can compare
    two changes by their cost and not only by their results.
13. As a maintainer, I want to keep tasks of a stage together in a benchmark, so
    that I can run them together.
14. As a maintainer, I want to run a benchmark under several configurations in
    the UI and see their results side by side, so that I can tell which
    change helps across real work.
15. As a maintainer, I want to run each task more than once, so that a lucky
    or unlucky trial does not decide.
16. As an operator, I want benchmark runs to let the runs of the pipeline go
    first, so that benchmarking does not keep the factory waiting.

## Tasks

A task holds the input of one run: the items, the repository and the
repository's properties, as they were when the run started. It also holds
what a good result is.

| Part         | Holds                                                                                                                                                         |
| ------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| stage        | the run's stage                                                                                                                                               |
| item         | the item the run worked on                                                                                                                                    |
| start        | when Linewright decided to start the run                                                                                                                      |
| items        | the items from which Linewright fills the prompt of the stage ([Contract](runs.md#contract)), each with the comments written before the start |
| repository   | the default branch's head at the start and, for a run on a branch that held earlier work, the branch's head, with their history, which holds the spec change's commits |
| properties   | the repository's properties ([The repository's properties](repository.md#the-repositorys-properties)) |
| criteria     | what a good result is, in the capturer's words |
| run          | the ID of the run it was captured from                                                                                                                        |

- **The stage.** A task is a task of its stage: the stage is run on it again
  whatever Linewright would now decide to start on its item.
- **As GitHub and git give it.** Linewright keeps the items and the
  properties as GitHub's API gives them, and the commits as git gives them. A
  change to how Linewright reads them then leaves the task usable.
- **The repository.** A bare git repository in the task.
- **The criteria.** Criteria that hold only white space are no criteria.
- **A directory.** A task is a directory holding all of it, with its
  criteria as a file in it.

## Recording a run's input

Each triage, design and implement run records its input at its start, as
GitHub gave it then: the items, the properties and the heads of its branches.
Linewright keeps the record for as long as it keeps the run
([Runs](runs.md#ending)).

## Capturing

Capturing a triage, design or implement run that recorded its input writes a
task of it, whatever its outcome.

- **Where.** In `tasks/` under the work directory, in a directory named for the
  task.
- **Into a benchmark.** A capture puts the task into a benchmark of the run's
  stage ([Benchmarks](#benchmarks-1)).
- **From the UI.** [Run](runs.md#run) and [New task](#new-task).
- **From the command line.**
  `lw capture ID NAME --benchmark BENCHMARK --criteria FILE`. It works whether
  or not `serve` is running, and exits as the monitoring commands do
  ([Output](operation.md#output)).
- **Without criteria.** Linewright refuses a capture that has no criteria.
- **The name.** Given by the capturer: lowercase letters, digits and hyphens,
  starting with a letter or a digit. A name a task already has is refused.
- **The commits.** Fetched from GitHub.
- **All or nothing.** A capture that fails, such as for a commit GitHub no
  longer has, writes nothing and says why.
- **Before the first poll.** In the UI, Linewright refuses a capture until its
  first poll since `serve` started.
- **Refusals on a screen.** [New task](#new-task) and [Run](runs.md#run) give
  these messages for a capture's refusals. A name that is empty gets the message
  for no name where there is one, and not the message for a name out of form.

  | Refusal                                              | Message                                                       |
  | ---------------------------------------------------- | ------------------------------------------------------------- |
  | a name that a task has                               | "A task named NAME already exists."                           |
  | no criteria                                          | "A task needs criteria."                                      |
  | a name that is not in the form of a name             | as on [New benchmark](#new-benchmark)                         |
  | a run of a stage other than the benchmark's          | "Run ID is of STAGE, and BENCHMARK holds tasks of STAGE."     |
  | a run that recorded no input                         | "Run ID recorded no input."                                   |
  | a run whose record lacks a part                      | "The record of run ID lacks PART."                            |
  | a run that does not exist                            | "No such run ID."                                             |
  | a capture before Linewright's first poll             | "Linewright can't capture a run before its first poll."       |

## Benchmarks

A benchmark is a named set of tasks of one stage. A maintainer makes a benchmark
in the UI, captures runs of its stage into it and runs benchmark runs on it.

- **The name.** In the form of the name of a task
  ([Capturing](#capturing)). A name that a benchmark already has is refused.
- **The stage.** Triage, design or implement, given as the benchmark is made.
- **One benchmark for each task.** A task belongs to the benchmark it was
  captured into. A run captured into two benchmarks gives two tasks, each with
  its own criteria.
- **The order of its tasks.** The order in which they were captured into it,
  oldest first.
- **Removing a task.** Removing a task from its benchmark removes the task.

## Trials

A trial runs the task's stage on the task as `serve` runs the stage.
Linewright reads from the task and writes to the trial's output, in place of
GitHub.

- **From the command line.** The command is `lw trial TASK --out DIR`. TASK
  is the directory of the task, and DIR is the directory of the output. The
  command needs both.
- **Startup.** At startup, a trial makes only the checks of its settings
  and of the operator's layer of the brief
  ([Startup](repository.md#startup)). For a trial, each of
  Linewright's labels is present.
- **The environment.** A trial builds its environment from the head of the
  task's default branch. A later trial from the same head uses that
  environment, unless the setup command, or a variable that it runs with, has
  changed since the build ([Building](sessions.md#building)).
- **A build in progress.** A trial that needs an environment that another
  trial is building waits for that build.
- **Old environments.** As a trial starts, it removes each environment of a
  trial that is older than `setup.max-age` and that no trial is using.
- **A failed build.** A build that fails ends the trial before its run.
- **The clone of a session.** Its `origin` is a copy of the task's
  repository.
- **What GitHub decides.** In a trial, each write succeeds as a write that
  GitHub allows, such as an assignment.
- **Where Linewright keeps the run.** Linewright keeps the run of a trial in
  the trial's output only. The run records no input: its input is the task.
- **`max-runs`.** `serve` does not count the run of a trial toward
  `max-runs`.
- **A task that Linewright cannot run.** Linewright refuses a task that
  lacks a part, or that holds a part that it cannot read.
  The refusal names the part.
- **A file with an error.** Linewright refuses a task when a file on its
  default branch would hold the run under `serve`
  ([Linewright's files](repository.md#linewrights-files)). The refusal names
  the file and the error.
- **A signal.** A trial that a signal ends
  ([Stopping](operation.md#stopping)) leaves in its output what its run did
  until then.
- **The exit status of the command.** The command exits as the monitoring
  commands do ([Output](operation.md#output)). It succeeds when the run
  ended, whatever the outcome of the run.

## Configurations

A configuration is what a trial changes: files of the brief and settings of
the sessions of its run. A configuration overrides every other layer, flags
included. Each file or setting that it does not hold comes from the other
layers.

- **From the command line.** `--configuration CONFIG` gives `lw trial` a
  configuration. CONFIG has the same layout as a repository's `.linewright/`:
  the settings in `config.toml` and the files of the brief in `briefs/`.
- **Files of the brief.** A configuration can hold each file that a layer of
  the brief can hold ([Briefs](operation.md#briefs)).
- **Settings.** `config.toml` holds only `model`, `effort` and
  `session-timeout`, with no tables. A setting in it overrides that setting
  in every stage table.
- **The scorer.** A configuration applies to the sessions of the run, not to
  the session of a scorer.
- **An error.** Each file that Linewright refuses in a layer of the brief,
  each other file in CONFIG and each other key in `config.toml` is an error,
  hidden files aside. Linewright refuses a configuration with an error before
  the trial starts. The refusal names the file and the error.

## The repository on GitHub

In a trial, the `gh` of a session cannot read the task's repository on
GitHub.

- **A request that names the repository.** A request names the task's
  repository in one of two ways:
  - it holds the whole owner and the whole name together, as `OWNER/NAME`;
  - its body holds the owner and the name, each as a whole quoted value.

  Letter case and percent-encoding make no difference. A request for a
  repository with a longer name, such as `OWNER/NAME-docs`, does not name it.
- **Requests of `gh`.** Linewright refuses each request of a session's `gh`
  that names the task's repository. Linewright passes every other request to
  GitHub unchanged, and passes the answer back unchanged.
- **The refusal.** The refusal tells the agent that the repository on GitHub
  is not available in a trial.

## The output of a trial

The output of a trial holds what the trial did and how a scorer scored it,
as plain files.

| Part       | Holds                                                                                                                    |
| ---------- | ------------------------------------------------------------------------------------------------------------------------ |
| outcome    | the outcome of the run ([Ending](runs.md#ending))                                                                        |
| sessions   | the record of each session that Linewright keeps of a run ([Ending](runs.md#ending)), as files                           |
| writes     | each write that Linewright would have made to GitHub, in order, such as a comment, a label, a ticket or the pull request |
| commits    | the branch that Linewright would have pushed, if the run pushes one, in a git repository                                 |
| build      | the output and the time of the build of the environment, if the trial built it                                          |
| cost       | the time of the trial from its start to its end, with the build, and the cost of its run, if it has a run ([Cost](runs.md#cost)) |
| Linewright | the version of Linewright that ran the trial                                                                            |
| task       | the ID of the run from which the task of the trial was captured |
| configuration | a copy of the configuration of the trial, if it had one ([Configurations](#configurations)) |
| score  | the score of the trial, if a scorer made one ([Scoring](#scoring)) |

- **Where.** Linewright makes the directory of the output if there is none,
  and refuses a directory that holds anything.
- **Attachments.** An attachment that a write refers to is a file in the
  output. The Markdown of the write refers to the file by a relative path.
- **Numbers.** An issue or pull request that the run opens takes the next
  number above the numbers of the task's items.

## Scoring

A scorer scores the output of a trial against the criteria of its task. It
also says whether a session of the trial learned how the work turned out.
The scorer is an agent in a session of its own.

- **From the command line.** The command is `lw score TASK DIR`. TASK is the
  directory of the task, and DIR is the output of a trial of that task.
- **Which trials.** Linewright runs the scorer on a trial whose run ended
  done or handed back, and refuses every other trial.
- **The output and the task.** Linewright refuses an output that has no
  `task` part, or whose `task` part names a run other than the run of the
  task.
- **As a trial.** The rules of [Trials](#trials) hold for `lw score`
  as for `lw trial`, with the session of the scorer in place of the run. The
  exceptions are the rules on the command line, on writes, on where
  Linewright keeps the run, on a signal and on the exit status.
- **The `gh` of the scorer.** The session of the scorer has the `gh` of a
  session of a trial
  ([The repository on GitHub](#the-repository-on-github)).
- **The clone of the scorer.** Linewright checks out, from the commits of the
  output, the branch that it would have pushed, if the run pushes one. If the
  run pushes none, the clone is at the head of the task's default branch.
- **What the scorer reads.** Each session of the scorer reads the trial from a
  copy that Linewright makes for that session, in a directory apart from the
  output. The copy holds the outcome, the sessions' records, the writes and the
  attachments that the writes refer to, each at its path in the output. It holds
  nothing else of the output. Linewright removes the copy as it removes the
  clone of the session ([Stages](runs.md#stages)). A change that the scorer
  makes to the copy does not change the output.
- **The prompt.** Linewright sends the scorer the prompt `prompts/scorer.md`
  ([Briefs](operation.md#briefs)), with these placeholders filled:

  | placeholder    | value                                                                                    |
  | -------------- | ---------------------------------------------------------------------------------------- |
  | `{{facts}}`    | as in every prompt ([Contract](runs.md#contract))                                        |
  | `{{stage}}`    | the stage of the task                                                                    |
  | `{{criteria}}` | the criteria of the task                                                                 |
  | `{{trial}}`   | where the outcome, the sessions' records and the writes are in the scorer's copy (`trial-output.md`) |
  | `{{output}}`   | the result contract of the scorer, `briefs/outputs/scorer.md`, with nothing on attachments |

- **The result.** The session of the scorer ends with a result that matches
  `briefs/results/scorer.schema.json`. Linewright handles that result, and a
  failure of the session, as it does for a session of a stage
  ([Contract](runs.md#contract), [Ending](runs.md#ending)), but it hands no
  item back.
- **The score.** Linewright keeps the score in the output of the trial, apart
  from the sessions of the trial. The score holds these parts:

  | Part        | Holds                                                                                                                      |
  | ----------- | -------------------------------------------------------------------------------------------------------------------------- |
  | correctness | `pass` if the output meets every criterion, and `fail` if it does not, with the reasoning of the scorer                    |
  | leak        | whether a session of the trial learned how the work turned out, with the reasoning of the scorer and the tool calls that show it |
  | session     | the record of the session of the scorer                                                                                     |
  | Linewright  | the version of Linewright that ran the scorer                                                                               |

- **An output with a score.** Linewright refuses to score an output that
  holds a score.
- **A scorer that makes no score.** The scorer makes no score in these cases.
  The command then says why, and keeps nothing of that scorer. The first row
  that fits gives why:

  | The scorer makes no score if | Why |
  | --- | --- |
  | a signal ends the command | "A signal ended the scorer." |
  | its session ends for the usage limit ([Usage limit](runs.md#usage-limit)) | "The scorer's session reached the usage limit, which resets at TIME." |
  | it gives up | "The scorer gave up: REASON", where REASON is the reason the scorer gives |
  | its session ends without a result that Linewright accepts | "The scorer's session ended without a score: REASON", where REASON is the reason for the end ([Ending](runs.md#ending)), then the tail of its log |
- **The exit status of the command.** The command succeeds when the scorer
  made a score, whatever the score.

## Benchmark runs

A benchmark run makes a trial of each task of a benchmark under each of its
configurations, and scores each trial. Its repetitions are how many times it
makes a trial of each task under each configuration.

- **From the UI.** [New benchmark run](#new-benchmark-run) launches a benchmark
  run, and [Benchmark run](#benchmark-run) follows it, shows its results and
  cancels it.
- **The configurations.** The first configuration is `as-is`, which holds
  nothing ([Configurations](#configurations)). The maintainer adds the others,
  each with a name that no other configuration of the benchmark run has, in the
  form of the name of a task ([Capturing](#capturing)).
- **Repetitions.** One or more. The default is 1.
- **At its launch.** The benchmark run keeps the tasks of the benchmark with
  their criteria, the configurations and the repetitions. A later change to the
  benchmark or to a task does not change the benchmark run. Linewright refuses a
  launch on a benchmark with no tasks, or with a configuration with an error.
- **Under `serve`.** `serve` runs the trials and the scorers of a benchmark run,
  each as `lw trial` and `lw score` do. A drain, the usage limit and stopping
  apply to each trial and scorer as to a run of `serve` ([Holds](runs.md#holds),
  [Usage limit](runs.md#usage-limit), [Stopping](operation.md#stopping)). The
  other holds do not hold them. A benchmark run that `serve` does not run waits.
- **Order.** The trials start one repetition at a time. In a repetition, they
  start task by task, in the order of the benchmark's tasks
  ([Benchmarks](#benchmarks-1)), and for each task, configuration by
  configuration. Thus all the configurations have trials of the same tasks when
  a benchmark run stops early.
- **Trials at once.** At most `max-trials` trials and scorers of benchmark runs
  run at once. None starts while a run of the pipeline waits for a place
  under `max-runs`.
- **A trial without a score.** A trial has no score when its build fails, when
  its run is not done or handed back, or when its scorer makes no score. The
  benchmark run shows the reason, and Linewright does not run the trial again.
- **Interrupted trials.** A trial whose run or scorer a restart interrupts has
  no reason. It starts again ([After a restart](#benchmark-runs)).
- **Cancelled trials.** When the maintainer cancels a benchmark run, each of its
  trials with no score and no reason is cancelled, and the run of each of its
  trials in flight stops and has ended. A cancelled trial has no reason, also
  when the cancel stops its run or its scorer. The trial keeps what its run cost
  until it stopped. The figures of a configuration ([Results](#results)) do not
  count a trial whose run stopped then or did not start.
- **After a restart.** Once `serve` runs, the benchmark run starts again each
  trial and each scorer that the restart interrupted, a trial into a new
  output.
- **The output.** The output of each trial of benchmark run ID is in
  `benchmark-runs/ID/CONFIGURATION/TASK/REPETITION/` under the work directory
  ([The output of a trial](#the-output-of-a-trial)).

| State     | Event                                      | Next      | Effect                                  |
| --------- | ------------------------------------------ | --------- | --------------------------------------- |
| running   | a hold holds its next trial or scorer      | waiting   | the benchmark run shows what it waits for  |
| running   | its next trial or scorer waits for a place under `max-trials`, or for a run of the pipeline to get one under `max-runs` | waiting | the benchmark run shows what it waits for |
| waiting   | the hold ends, or a place frees up         | running   | the next trial or scorer starts         |
| running   | its last trial or scorer ends              | ended     | its results are complete                |
| running   | the maintainer cancels it                  | cancelled | its trials and scorers in flight stop   |
| waiting   | the maintainer cancels it                  | cancelled | its trials and scorers in flight stop   |

### Results

The results of a benchmark run give these figures for each configuration.

| Figure    | For a configuration                                                                                       |
| --------- | --------------------------------------------------------------------------------------------------------- |
| pass rate | of its trials with a score and no leak, the share whose correctness is `pass`                         |
| passes    | for each task, how many of its trials of that task pass, of those with a score and no leak            |
| cost      | the mean and the total of the cost of its trials that have a cost ([Cost](runs.md#cost)), and how many have none |
| time      | the mean and the total of the time of its trials                                                         |
| leaks     | how many of its trials have a score with a leak                                                      |
| unscored  | how many of its trials have no score for a reason that [A trial without a score](#benchmark-runs) gives |

- **No trials to count.** A pass rate, the passes of a task, or a mean,
  with no trials to count is absent.
- **The scorers.** The cost of the scorers, from the records of their sessions,
  is a figure of the benchmark run, apart from the configurations.

## Benchmarks and benchmark runs on the command line

The commands work whether or not `serve` is running, and exit as the
monitoring commands do ([Output](operation.md#output)). A benchmark, a task or a
benchmark run that does not exist is as an item or run that does not exist.

| Command                                                                     | What it does                                                    |
| --------------------------------------------------------------------------- | --------------------------------------------------------------- |
| `lw benchmarks`                                                                 | list the benchmarks                                                 |
| `lw benchmark add NAME STAGE`                                                   | make a benchmark                                                    |
| `lw benchmark show NAME`                                                        | show a benchmark: its tasks with their criteria, and its benchmark runs |
| `lw benchmark remove-task NAME TASK`                                            | remove a task from a benchmark                                      |
| `lw benchmark-run launch BENCHMARK [--configuration NAME=DIR]... [--repetitions N]` | launch a benchmark run, with `as-is` and a configuration for each DIR ([Configurations](#configurations)) |
| `lw benchmark-run show ID`                                                     | show a benchmark run: its state, its trials and its results      |
| `lw benchmark-run cancel ID`                                                   | cancel a benchmark run                                            |

## Screens

| Screen            | Reached from                                                    | Purpose                                                                                    |
| ----------------- | --------------------------------------------------------------- | ------------------------------------------------------------------------------------------ |
| Benchmarks            | the navigation ([Shell](shell.md))                              | find a benchmark                                                                               |
| New benchmark         | "New" on Benchmarks                                                 | make a benchmark                                                                               |
| Benchmark             | a benchmark on Benchmarks                                               | see a benchmark's benchmark runs and tasks                                                        |
| New task          | "New" on Benchmark's tasks                                          | capture a run into the benchmark                                                               |
| New benchmark run    | "New" on Benchmark's benchmark runs                                    | launch a benchmark run on the benchmark                                                          |
| Benchmark run        | a benchmark run on Benchmark, and a benchmark's last benchmark run on Benchmarks | follow a benchmark run, see its results, and cancel it                                       |
| Remove task       | "Remove" on a task, on Benchmark                                    | confirm removing the task                                                                  |
| Cancel benchmark run | "Cancel", on Benchmark run                                         | confirm cancelling the benchmark run                                                          |

### Benchmarks

- **Layout.** The table "Benchmarks" ([Tables](design_system.md#tables)), with
  the filters and "New" ([Making](design_system.md#making)).
- **Filters.** Filters for the benchmark's name and its stage narrow the list.
  The filter for the name keeps each benchmark whose name holds the text typed,
  whatever its letter case.
- **The table.** A row per benchmark, in the order of their names.

  | Column          | Holds                                                |
  | --------------- | ---------------------------------------------------- |
  | Benchmark       | the benchmark's name                                 |
  | Stage           | the benchmark's stage                                    |
  | Last benchmark run | when its last benchmark run was launched, linking to it, and nothing for a benchmark with none |

### New benchmark

- **Layout.** A dialog titled "New benchmark"
  ([Making](design_system.md#making)).
- **Fields.** "Name" and "Stage" ([Benchmarks](#benchmarks-1)), then "Create".
  "Stage" starts on the first stage, triage.
- **Refusals.** The message for a name that a benchmark has is "A benchmark
  named NAME already exists." The message for a name that is not in the form of
  a name ([Capturing](#capturing)) is "A name holds only lowercase letters,
  digits and hyphens, and starts with a letter or a digit."

### Benchmark

- **Layout.** The heading at the top, then the benchmark's benchmark runs and
  its tasks, each in a tab of its own.
- **Heading.** The benchmark's name, with its stage under it.
- **Tabs.** "Runs", then "Tasks". The screen opens on Runs,
  or on Tasks while the benchmark has no benchmark runs.
- **Runs.** A table that fills the tab
  ([Tables](design_system.md#tables)), with a row per benchmark run, latest
  first, and "New" on a benchmark with tasks
  ([Making](design_system.md#making)).

  | Column         | Holds                                           |
  | -------------- | ----------------------------------------------- |
  | Launched       | when it was launched                               |
  | State          | its state ([Benchmark run](#benchmark-run))           |
  | Tasks          | how many tasks it makes trials of  |
  | Configurations | how many configurations it has                  |
  | Duration       | its duration ([Benchmark run](#benchmark-run))        |
  | Cost           | its cost ([Benchmark run](#benchmark-run))            |

- **Tasks.** A table that fills the tab ([Tables](design_system.md#tables)),
  with a row per task in the benchmark's order ([Benchmarks](#benchmarks-1)),
  and "New" ([Making](design_system.md#making)). A task's row opens to the whole
  of its criteria.

  | Column        | Holds                                                         |
  | ------------- | ------------------------------------------------------------- |
  | Task          | the task's name, with the first line of its criteria under it |
  | Captured from | the run it was captured from, linking to its Run              |
  |               | "Remove"                                                      |

### New task

- **Layout.** A dialog titled "New task in NAME"
  ([Making](design_system.md#making)).
- **Fields.** The fields below, then "Add task".

  | Field    | Holds                                                                                                      |
  | -------- | ---------------------------------------------------------------------------------------------------------- |
  | Run      | a choice of the runs of the benchmark's stage that Linewright can capture ([Capturing](#capturing)), latest first, or "No runs" when there is none, and then "Add task" is disabled |
  | Name     | the task's name, which Linewright makes from the run's ID as on Run ([Run](runs.md#run)) |
  | Criteria | the criteria                                                                                               |

### New benchmark run

- **Layout.** A dialog titled "New benchmark run on NAME"
  ([Making](design_system.md#making)).
- **Fields.** "Configurations", then "Repetitions"
  ([Benchmark runs](#benchmark-runs)), then how many trials they make, such as
  "2 configurations × 4 tasks × 3 repetitions: 24 trials". Then "Run benchmark".
- **Configurations.** Each configuration but `as-is` has "Name", and can set
  "model", "effort" and "session-timeout", and give the text of any file of the
  brief. Each such configuration has "Remove". "Add a configuration" adds one at
  the end.
- **Files of the brief.** A configuration has "Files of the brief", a choice of
  the files that a layer of the brief can hold ([Briefs](operation.md#briefs)).
  When the maintainer chooses one, a box for its text opens. Its text starts as
  the text that a run of the stage uses when the dialog opens. A file whose text
  the maintainer does not change is not part of the configuration.
- **Refusals.** A refusal names a configuration by its name. While the
  configuration has no name, the refusal names it by its place in the dialog,
  where `as-is` is the 1st. A name that is empty gets the message for no name.
  The messages are:

  | Refusal                                                        | Message                                                    |
  | -------------------------------------------------------------- | ---------------------------------------------------------- |
  | a benchmark with no tasks                                      | "The benchmark NAME has no tasks."                         |
  | a configuration with no name                                   | "The 3rd configuration needs a name.", with its place      |
  | a name that is not in the form of a name                       | as on [New benchmark](#new-benchmark)                      |
  | two configurations with the same name                          | "Two configurations are named NAME."                       |
  | a configuration with an error ([Configurations](#configurations)) | "The configuration NAME has an error in FILE: ERROR"    |
  | repetitions that are not a whole number, 1 or more              | "Repetitions are a whole number, 1 or more."               |

### Benchmark run

- **Layout.** The heading at the top, then the benchmark run's facts, then its
  results ([Results](#results)): the chart, the table and the grid.
- **Heading.** Benchmark run ID and its state. While the benchmark run is
  running or waiting, "Cancel" is with the screen's buttons
  ([Shell](shell.md)).
- **State.** The state shows as a status
  ([Status](design_system.md#status)): running as in flight, waiting as
  paused, ended as done and cancelled as cancelled.
- **Facts.** The benchmark run's facts are side by side in a row, each with its
  label above its value.

  | Fact        | Holds                                                                                   | Shown                          |
  | ----------- | --------------------------------------------------------------------------------------- | ------------------------------ |
  | Launched    | when the benchmark run was launched                                                        | always                         |
  | Ended       | when it ended or was cancelled                                                          | once it has ended or cancelled |
  | Duration    | the time from its launch to its end or its cancelling; while it runs or waits, the time up to now         | always                         |
  | Trials     | how many of its trials have ended, of all its trials. A trial has ended once its run has ended, whether or not its scorer has. | always |
  | Cost        | the sum of the cost of the tokens of all its trials and of its scorers ([Cost](runs.md#cost)). After the sum, how many of its trials that have ended have no cost, if any. | always |
  | Scorers      | the cost of the tokens of its scorers ([Results](#results)) | always |
  | Waiting for | what it waits for, such as "the usage limit, until 14:30"                               | while it waits                 |

- **The chart.** A point for each configuration, labelled with its name. The
  vertical axis is the pass rate. The horizontal axis is the mean cost of a
  trial or its mean time. The reader chooses which above the chart, at its
  left, after "Pass rate vs": "Mean cost" or "Mean time". It starts as "Mean
  cost".
- **A point.** On hover, a point shows its configuration's name, pass rate
  and mean. A configuration with no pass rate, or no mean on the horizontal
  axis, has no point. A chart with no points shows "No results".
- **The table.** "Configurations": a row for each configuration, highest pass
  rate first. Configurations with the same pass rate are in the order of the
  benchmark run. Those with no pass rate are last, in the order of the benchmark
  run. A configuration's row opens to a row for each task. A task's row opens to
  a row for each of its trials. Tasks and trials are in the order in which the
  benchmark run starts them ([Benchmark runs](#benchmark-runs)).

  | Column     | For a configuration                         | For a task                                  | For a trial                                                                              |
  | ---------- | ------------------------------------------- | ------------------------------------------- | ----------------------------------------------------------------------------------------- |
  | Configuration | its name                                 | the task's name                             | its repetition, such as "Trial 2"                                                        |
  | Pass rate  | its pass rate                               | its passes, such as "2 of 3"                | its state's name ([A trial's status](#benchmark-run))                                          |
  | Mean cost  | the mean of its cost                        | the same, of its trials of the task        | its cost                                                                                  |
  | Total cost | the total of its cost, and how many of its trials have no cost | the same, of its trials of the task      |                                                                                           |
  | Mean time  | the mean of its time                        | the same, of its trials of the task        | its time                                                                                  |
  | Total time | the total of its time                       | the same, of its trials of the task        |                                                                                           |
  | Leaks      | its leaks                                   | its trials of the task with a leak         |                                                                                           |
  | Unscored   | its unscored trials                        | its unscored trials of the task            |                                                                                           |

- **A trial's status.** A trial's Pass rate shows as a status
  ([Status](design_system.md#status)). The first row that fits the trial gives
  the name of its state and its status:

  | The trial | Name | Shows as |
  | --- | --- | --- |
  | it is cancelled ([Cancelled trials](#benchmark-runs)) | "cancelled" | cancelled |
  | a restart interrupted its run or its scorer, which has not started again | "waiting" | paused |
  | its build failed | "no score" | failed |
  | its run ended, but not done or handed back | "no score" | as its run's outcome does ([Ending](runs.md#ending)) |
  | its scorer made no score | "no score" | failed |
  | its score passes, with no leak | "pass" | done |
  | its score fails, with no leak | "fail" | failed |
  | its score has a leak | "leak" | failed |
  | its scorer is running | "scoring" | in flight |
  | its run ended done or handed back | "waiting" | paused |
  | its build or its run is running | "running" | in flight |
  | it has not started | "waiting" | paused |

- **A trial's row** opens to the trial's details. A screen reader reads its
  button as "Show the details of trial N of TASK under CONFIGURATION".
- **A trial's score.** If the trial has a score ([Scoring](#scoring)), its row
  opens to a line for each of these parts:

  | Label | Value |
  | --- | --- |
  | "Correctness" | "pass" or "fail", then the reasoning of the scorer |
  | "Leak" | "yes" or "no", then the reasoning of the scorer and the tool calls that show it |

- **A trial's reason.** If the trial has a reason for no score
  ([A trial without a score](#benchmark-runs)), its row opens to it:

  | It has no score, as | The reason |
  | --- | --- |
  | its build failed | "No score. The build of its environment failed." |
  | its run is not done or handed back | "No score. The run's outcome: OUTCOME", where OUTCOME is the run's outcome ([Ending](runs.md#ending)) |
  | its scorer made none | "No score.", then why its scorer made none ([Scoring](#scoring)) |

- **A trial's transcripts.** A trial's row also opens to the transcripts of its
  run and of its scorer, as Run shows a run's, once each has started. Each is
  under its heading, "Run" or "Scorer".
- **The grid.** "Passes by task": a row for each task, under the head "Task",
  and a column for each configuration, in the order of the table. A cell shows
  the passes of the task under the configuration, such as "2 of 3". A cell also
  shows its leaks and its trials without a score, if it has any.

### Remove task

- **Layout.** A confirmation
  ([Confirming](design_system.md#confirming)): "Remove the task NAME and its
  directory?"

### Cancel benchmark run

- **Layout.** A confirmation
  ([Confirming](design_system.md#confirming)): "Cancel benchmark run ID? This
  stops its running trials and scorers."

## Out of scope

- Capturing a rework, fix-ci or update run, or an automation's run.
- Capturing a run whose record lacks a part, by reading that part from GitHub
  as it is now.
- Stopping a session of a trial from reading the task's repository on GitHub
  by a means other than `gh`.
- Stopping a session of a trial from reading a copy, elsewhere, of the task's
  repository on GitHub, such as a fork or a search result.
- Refusing a request that holds only the ID that GitHub gives the task's
  repository.
- A configuration that changes the setup, the maintenance command or the
  variables of a session.
- Scoring a trial against anything other than the criteria of the task.
- Combining the scores of several scorers on one trial.
- Checking a score against the score of a person.
- Removing a benchmark or a benchmark run.
- Recommending a configuration.
- Saying whether a difference between configurations is more than chance.
- Copying the files or settings of a configuration into the repository or
  into the operator's settings.
- Adding to a benchmark a task that no benchmark holds.
- Copying a task or a benchmark to another Linewright.
