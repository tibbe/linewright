# Benchmarks

A benchmark measures a stage against work it has already done. A run of the
stage is captured as a case, which holds what the run was given as it stood
when the run started, so that the stage can be run on it again.

For example, a maintainer finds that the design of #812 cut the work badly.
They capture its design run as a case from the run's page and write what a
good design would have done. They then replay the case with each change they
make to the brief of the design stage.

## User Stories

1. As an operator, I want to capture a run of a stage from its page, so that I
   can run that stage on the same input again.
2. As an operator, I want a case to hold only what its run was given and
   nothing from after it started, so that the stage run on it again has
   nothing that shows how the work turned out.
3. As an operator, I want a case to stay usable as Linewright changes, so that
   cases captured long ago still measure the stage as it is today.
4. As an operator, I want to say what a good result is as I capture a case, so
   that results can be judged against it.
5. As an operator, I want to copy a case to another machine, so that I run it
   where I tune the briefs.
6. As an agent, I want to capture a case from the command line, so that I can
   build cases without a browser.
7. As an operator, I want to run a case's stage again with a setting or its
   brief changed, so that I see what the change does to real work.
8. As an operator, I want a replay to write nothing to GitHub, so that I can
   try a change without touching the repository.
9. As an agent, I want a replay's output as plain files, so that I can read
   it, judge it and change the briefs.

## Cases

A case holds the input of one run: the items, the repository and the
repository's properties, as they were when the run started. It also holds
what a good result is.

| Part         | Holds                                                                                                                                                         |
| ------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| stage        | the run's stage                                                                                                                                               |
| item         | the item the run worked on                                                                                                                                    |
| start        | when Linewright decided to start the run                                                                                                                      |
| items        | the item, its parent issue, the spec change it was opened for or cut from, and the spec gaps filed for it and still open, each that it has: its title, body, author, labels, type, state, the comments written before the start, and its links to the others as GitHub records them |
| repository   | the default branch's head at the start and, for a run on a branch that held earlier work, the branch's head, with their history, which holds the spec change's commits |
| properties   | the repository's properties ([The repository's properties](repository.md#the-repositorys-properties)) |
| criteria     | what a good result is, in the capturer's words, or nothing                                                                                                   |
| run          | the ID of the run it was captured from                                                                                                                        |

- **The stage.** A case is a case of its stage: the stage is run on it again
  whatever Linewright would now decide to start on its item.
- **As GitHub and git give it.** Linewright keeps the items and the
  properties as GitHub's API gives them, and the commits as git gives them. A
  change to how Linewright reads them then leaves the case usable.
- **The repository.** A bare git repository in the case.
- **A directory.** A case is a directory holding all of it, so that it is
  copied, moved or removed as a directory, and its criteria are edited as a
  file in it.

## Recording a run's input

Each triage, design and implement run records its input at its start, as
GitHub gave it then: the items, the properties and the heads of its branches.
Linewright keeps the record for as long as it keeps the run
([Runs](runs.md#ending)).

## Capturing

Capturing a triage, design or implement run that recorded its input writes a
case of it, whatever its outcome.

- **Where.** In `cases/` under the work directory, in a directory named for the
  case.
- **From the web UI.** The run page
  ([The run page](design_system.md#the-run-page)).
- **From the command line.** `lw capture ID NAME`, with `--criteria FILE` for
  the criteria. It works whether or not `serve` is running, and exits as the
  monitoring commands do ([Output](monitoring.md#output)).
- **The name.** Given by the capturer: lowercase letters, digits and hyphens.
  A name a case already has is refused.
- **The commits.** Fetched from GitHub.
- **All or nothing.** A capture that fails, such as for a commit GitHub no
  longer has, writes nothing and says why.

## Replaying

A replay runs the case's stage on the case as `serve` runs the stage.
Linewright reads from the case and writes to the replay's output, in place of
GitHub.

- **From the command line.** The command is `lw replay CASE --out DIR`. CASE
  is the directory of the case, and DIR is the directory of the output. The
  command needs both.
- **Startup.** At startup, a replay makes only the checks of its
  configuration ([Startup](repository.md#startup)). For a replay, each of
  Linewright's labels is present.
- **The environment.** A replay builds its environment from the head of the
  case's default branch. A later replay from the same head uses that
  environment, unless the setup command, or a variable that it runs with, has
  changed since the build ([Building](sessions.md#building)).
- **A build in progress.** A replay that needs an environment that another
  replay is building waits for that build.
- **Old environments.** As a replay starts, it removes each environment of a
  replay that is older than `setup.max-age` and that no replay is using.
- **A failed build.** A build that fails ends the replay before its run.
- **The clone of a session.** Its `origin` is a copy of the case's
  repository.
- **What GitHub decides.** In a replay, each write succeeds as a write that
  GitHub allows, such as an assignment.
- **Where Linewright keeps the run.** Linewright keeps the run of a replay in
  the replay's output only. The run records no input: its input is the case.
- **`max-runs`.** `serve` does not count the run of a replay toward
  `max-runs`.
- **A case that Linewright cannot replay.** Linewright refuses a case that
  lacks a part other than its criteria, or that holds a part it cannot read.
  The refusal names the part.
- **A file with an error.** Linewright refuses a case when a file on its
  default branch would hold the run under `serve`
  ([Linewright's files](repository.md#linewrights-files)). The refusal names
  the file and the error.
- **A signal.** A replay that a signal ends
  ([Stopping](operation.md#stopping)) leaves in its output what its run did
  until then.
- **The exit status of the command.** The command exits as the monitoring
  commands do ([Output](monitoring.md#output)). It succeeds when the run
  ended, whatever the outcome of the run.

## The output of a replay

The output of a replay holds what the replay did, as plain files.

| Part       | Holds                                                                                                                    |
| ---------- | ------------------------------------------------------------------------------------------------------------------------ |
| outcome    | the outcome of the run ([Ending](runs.md#ending))                                                                        |
| sessions   | the record of each session, as Linewright keeps the record of a run ([Ending](runs.md#ending))                           |
| writes     | each write that Linewright would have made to GitHub, in order, such as a comment, a label, a ticket or the pull request |
| commits    | the branch that Linewright would have pushed, if the run pushes one, in a git repository                                 |
| build      | the output of the build of the environment, if the replay built it                                                       |
| Linewright | the version of Linewright that ran the replay                                                                            |

- **Where.** Linewright makes the directory of the output if there is none,
  and refuses a directory that holds anything.
- **Attachments.** An attachment that a write refers to is a file in the
  output. The Markdown of the write refers to the file by a relative path.
- **Numbers.** An issue or pull request that the run opens takes the next
  number above the numbers of the case's items.

## Out of scope

- Capturing a rework, fix-ci or update run, or an automation's run.
- Capturing a run whose record lacks a part, by reading that part from GitHub
  as it is now.
- Stopping the sessions of a replay from reading GitHub and the web, which
  show how the work turned out.
- Replaying a case from the web UI.
- Replaying a case several times, or several cases, with one command.
- Judging the output of a replay against the criteria of the case.
