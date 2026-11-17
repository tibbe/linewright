# Benchmarks

A benchmark measures a stage against work it has already done. A run of the
stage is captured as a case, which holds what the run was given as it stood
when the run started, so that the stage can be run on it again. A judge then
says whether what the stage made meets what the capturer said a good result
is.

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
8. As an operator, I want Linewright to write nothing to GitHub in a replay,
   so that I can try a change without touching the repository.
9. As an agent, I want a replay's output as plain files, so that I can read
   it, judge it and change the briefs.
10. As an operator, I want the `gh` of a replay's session kept from the case's
    repository on GitHub, so that the session cannot read how the work turned
    out.
11. As an operator, I want a judge to say whether the output of a replay
    meets the criteria of the case, so that I know whether a change helped
    and do not read every output.
12. As an operator, I want the judge to tell me when a session learned how
    the work turned out, so that I do not trust that replay.
13. As an operator, I want to run the judge on a replay again, so that I can
    change the judge and not replay the case.
14. As an operator, I want a replay to say what it cost, so that I can compare
    two changes by their cost and not only by their results.

## Cases

A case holds the input of one run: the items, the repository and the
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

- **The stage.** A case is a case of its stage: the stage is run on it again
  whatever Linewright would now decide to start on its item.
- **As GitHub and git give it.** Linewright keeps the items and the
  properties as GitHub's API gives them, and the commits as git gives them. A
  change to how Linewright reads them then leaves the case usable.
- **The repository.** A bare git repository in the case.
- **The criteria.** Criteria that hold only white space are no criteria.
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
- **From the command line.** `lw capture ID NAME --criteria FILE`. It works
  whether or not `serve` is running, and exits as the monitoring commands do
  ([Output](monitoring.md#output)).
- **Without criteria.** Linewright refuses a capture that has no criteria.
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
  lacks a part, or that holds a part that it cannot read.
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

## The repository on GitHub

In a replay, the `gh` of a session cannot read the case's repository on
GitHub.

- **A request that names the repository.** A request names the case's
  repository in one of two ways:
  - it holds the whole owner and the whole name together, as `OWNER/NAME`;
  - its body holds the owner and the name, each as a whole quoted value.

  Letter case and percent-encoding make no difference. A request for a
  repository with a longer name, such as `OWNER/NAME-docs`, does not name it.
- **Requests of `gh`.** Linewright refuses each request of a session's `gh`
  that names the case's repository. Linewright passes every other request to
  GitHub unchanged, and passes the answer back unchanged.
- **The refusal.** The refusal tells the agent that the repository on GitHub
  is not available in a replay.

## The output of a replay

The output of a replay holds what the replay did and how a judge judged it,
as plain files.

| Part       | Holds                                                                                                                    |
| ---------- | ------------------------------------------------------------------------------------------------------------------------ |
| outcome    | the outcome of the run ([Ending](runs.md#ending))                                                                        |
| sessions   | the record of each session that Linewright keeps of a run ([Ending](runs.md#ending)), as files                           |
| writes     | each write that Linewright would have made to GitHub, in order, such as a comment, a label, a ticket or the pull request |
| commits    | the branch that Linewright would have pushed, if the run pushes one, in a git repository                                 |
| build      | the output and the time of the build of the environment, if the replay built it                                          |
| cost       | the time of the replay from its start to its end, with the build, and the cost of its run, if it has a run ([Cost](runs.md#cost)) |
| Linewright | the version of Linewright that ran the replay                                                                            |
| case       | the ID of the run from which the case of the replay was captured |
| judgement  | the judgement of the replay, if a judge made one ([Judging](#judging)) |

- **Where.** Linewright makes the directory of the output if there is none,
  and refuses a directory that holds anything.
- **Attachments.** An attachment that a write refers to is a file in the
  output. The Markdown of the write refers to the file by a relative path.
- **Numbers.** An issue or pull request that the run opens takes the next
  number above the numbers of the case's items.

## Judging

A judge judges the output of a replay against the criteria of its case. It
also says whether a session of the replay learned how the work turned out.
The judge is an agent in a session of its own.

- **From the command line.** The command is `lw judge CASE DIR`. CASE is the
  directory of the case, and DIR is the output of a replay of that case.
- **Which replays.** Linewright runs the judge on a replay whose run ended
  done or handed back, and refuses every other replay.
- **The output and the case.** Linewright refuses an output that has no
  `case` part, or whose `case` part names a run other than the run of the
  case.
- **As a replay.** The rules of [Replaying](#replaying) hold for `lw judge`
  as for `lw replay`, with the session of the judge in place of the run. The
  exceptions are the rules on the command line, on writes, on where
  Linewright keeps the run, on a signal and on the exit status.
- **The `gh` of the judge.** The session of the judge has the `gh` of a
  session of a replay
  ([The repository on GitHub](#the-repository-on-github)).
- **The clone of the judge.** Linewright checks out, from the commits of the
  output, the branch that it would have pushed, if the run pushes one. If the
  run pushes none, the clone is at the head of the case's default branch.
- **What the judge reads.** Each session of the judge reads the replay from a
  copy that Linewright makes for that session, in a directory apart from the
  output. The copy holds the outcome, the sessions' records, the writes and the
  attachments that the writes refer to, each at its path in the output. It
  holds nothing else of the output, so that the judge does not get an earlier
  judgement. Linewright removes the copy as it removes the clone of the
  session ([Stages](runs.md#stages)). A change that the judge makes to the
  copy does not change the output.
- **The prompt.** Linewright sends the judge the prompt `prompts/judge.md`
  ([Briefs](operation.md#briefs)), with these placeholders filled:

  | placeholder    | value                                                                                    |
  | -------------- | ---------------------------------------------------------------------------------------- |
  | `{{facts}}`    | as in every prompt ([Contract](runs.md#contract))                                        |
  | `{{stage}}`    | the stage of the case                                                                    |
  | `{{criteria}}` | the criteria of the case                                                                 |
  | `{{replay}}`   | where the outcome, the sessions' records and the writes are in the judge's copy (`replay-output.md`) |
  | `{{output}}`   | the result contract of the judge, `briefs/outputs/judge.md`, with nothing on attachments |

- **The result.** The session of the judge ends with a result that matches
  `briefs/results/judge.schema.json`. Linewright handles that result, and a
  failure of the session, as it does for a session of a stage
  ([Contract](runs.md#contract), [Ending](runs.md#ending)), but it hands no
  item back.
- **The judgement.** Linewright keeps the judgement in the output of the replay,
  apart from the sessions of the replay. The judgement holds these parts:

  | Part        | Holds                                                                                                                      |
  | ----------- | -------------------------------------------------------------------------------------------------------------------------- |
  | correctness | `pass` if the output meets every criterion, and `fail` if it does not, with the reasoning of the judge                    |
  | leak        | whether a session of the replay learned how the work turned out, with the reasoning of the judge and the tool calls that show it |
  | session     | the record of the session of the judge                                                                                     |
  | Linewright  | the version of Linewright that ran the judge                                                                               |

- **Judging again.** A new judgement replaces the earlier judgement.
- **A judge that makes no judgement.** The judge makes no judgement if it
  gives up, if a signal ends the command, or if its session ends without a
  result that Linewright accepts. The command then says why, keeps nothing of
  that judge, and leaves the earlier judgement in place.
- **The exit status of the command.** The command succeeds when the judge
  made a judgement, whatever the judgement.

## Out of scope

- Capturing a rework, fix-ci or update run, or an automation's run.
- Capturing a run whose record lacks a part, by reading that part from GitHub
  as it is now.
- Stopping a session of a replay from reading the case's repository on GitHub
  by a means other than `gh`.
- Stopping a session of a replay from reading a copy, elsewhere, of the case's
  repository on GitHub, such as a fork or a search result.
- Refusing a request that holds only the ID that GitHub gives the case's
  repository.
- Replaying a case from the web UI.
- Replaying a case several times, or several cases, with one command.
- Judging a replay against anything other than the criteria of the case.
- Combining the judgements of several judges on one replay.
- Checking a judgement against the judgement of a person.
- Preventing a judge from reading the earlier judgement outside its copy, such
  as in the directory of the output.
