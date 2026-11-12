# Benchmarks

A benchmark measures a stage against work it has already done. A run of the
stage is captured as a case, which holds what the run was given as it stood
when the run started, so that the stage can be run on it again.

For example, a maintainer finds that the design of #812 cut the work badly.
They capture its design run as a case from the run's page and write what a
good design would have done.

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

## Cases

A case holds one run's input, the items and repository its stage was given as
they were when the run started, with what a good result is.

| Part         | Holds                                                                                                                                                         |
| ------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| stage        | the run's stage                                                                                                                                               |
| item         | the item the run worked on                                                                                                                                    |
| start        | when Linewright decided to start the run                                                                                                                      |
| items        | the item, its parent issue, the spec change it was opened for or cut from, and the spec gaps filed for it and still open, each that it has: its title, body, author, labels, type, state, the comments written before the start, and its links to the others as GitHub records them |
| repository   | the default branch's head at the start and, for a run on a branch that held earlier work, the branch's head, with their history, which holds the spec change's commits |
| criteria     | what a good result is, in the capturer's words, or nothing                                                                                                   |
| run          | the ID of the run it was captured from                                                                                                                        |

- **The stage.** A case is a case of its stage: the stage is run on it again
  whatever Linewright would now decide to start on its item.
- **As GitHub and git give it.** The items are kept as GitHub's API gives them
  and the commits as git does, not as Linewright reads them, so that a change
  to how Linewright reads them leaves the case usable.
- **The repository.** A bare git repository in the case.
- **A directory.** A case is a directory holding all of it, so that it is
  copied, moved or removed as a directory, and its criteria are edited as a
  file in it.

## Recording a run's input

Each triage, design and implement run records its input at its start, as
GitHub gave it then, for as long as Linewright keeps the run
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

## Out of scope

- Capturing a rework, fix-ci or update run, or an automation's run.
- Capturing a run that recorded no input, by rebuilding its input from GitHub.
