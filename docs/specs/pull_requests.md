# Pull requests

The pull request is where the maintainer and the factory meet, and Linewright
works it like a careful contributor: it asks for review only once CI is green,
treats review as a conversation and answers in each thread, keeps the branch
mergeable, and takes a closed pull request as a no.

For example, the maintainer asks for a rename in a review comment. A rework
run makes the change, pushes, replies in the thread and asks the maintainer
to review again; the maintainer approves and merges.

## User Stories

1. As an operator, I want the PR to be a draft until CI passes, so that I am
   only asked to review green PRs.
2. As an operator, I want CI failures fixed by the agent, so that red CI costs
   me nothing.
3. As an operator, I want my review comments addressed and answered where I
   wrote them, so that review is a conversation with the branch.
4. As an operator, I want a PR's commits kept a coherent series where my
   repository merges them as they are, so that merging needs no round of
   cleaning up its history.
5. As an operator, I want branches kept mergeable by my repository's merge
   method and brought up to date with the default branch only when merging
   needs it, so that my review is not disturbed and CI does not rerun for
   nothing.
6. As an operator, I want to merge myself, so that nothing lands without me.
7. As an operator, I want closing a PR to mean rejection, so that the factory
   does not redo what I refused.
8. As an operator, I want to let Linewright merge its own green PRs, except
   those my repository requires a person to review, so that only the changes I
   have reserved for myself wait for me.

## The pull request

Once Linewright opens a PR, the PR is the item its work is on.

- **Opening.** When the implementer is done Linewright pushes its branch and
  opens a draft PR from it to the default branch: title and body written by
  the agent for the diff, which Linewright ends with `Closes #N` for the
  ticket.
- **Steering.** The runs that fix the PR's CI, address its feedback and keep
  it up to date are the PR's, and the PR is steered on the PR. The ticket's
  label and assignee don't steer it.
- **The issues it closes.** A PR is tied to issues only by GitHub's closing
  references, the closing keywords in its body and the issues linked to it by
  hand: it closes those issues.
- **Assignment.** Linewright works the PR while it is assigned to it, and a
  person steers it as under [Steering](repository.md#steering).
- **A person's commits.** Commits a person pushes to the branch are kept:
  every run starts from the branch as it is.
- **Description.** A run that changes a PR so its description no longer fits
  rewrites the description, keeping the issues it closes.
- **Closed without merging.** A PR Linewright opened that a person closes
  without merging is the maintainer's no ([Steering](repository.md#steering)).

## Working a pull request

Linewright works a pull request it opened as this table gives: at each poll,
the first row that holds ([Working an item](repository.md#working-an-item)).
Its state is GitHub's: a draft, or ready for review.

| State | When | Linewright | State after |
| --- | --- | --- | --- |
| any | feedback has come ([Rework](#rework)) | starts a rework run | unchanged |
| any | it conflicts with the default branch | starts an update run ([Keeping up to date](#keeping-up-to-date)) | unchanged |
| any | it cannot be merged by its merge method, for a reason other than being behind, and no run has ended on it since its last push | starts an update run | unchanged |
| any | none of its checks has appeared, and its last push was less than `ci-grace` ago | waits for its checks | unchanged |
| any | a check is still running `ci-timeout` after its last push, or after Linewright was last assigned to it where that is later | hands it back, naming the checks still running | unchanged |
| any | a check is still running | waits for its checks | unchanged |
| any | a check failed, and fewer than `ci-attempts` fix-ci runs have ended on it this round | starts a fix-ci run ([CI](#ci)) | unchanged |
| any | a check failed | hands it back, naming the failing checks | unchanged |
| any | it is behind the default branch, the repository requires it up to date to merge, and that alone keeps it from being ready to merge | starts an update run | unchanged |
| draft | otherwise | marks it ready for review | ready for review |
| ready for review | `auto-merge` is off, or a person holds its merge ([Merging](#merging)) | waits for review | ready for review |
| ready for review | two merges of it have failed this round | hands it back with GitHub's reason | ready for review |
| ready for review | GitHub blocks the merge, and Linewright has not commented on it this round naming what GitHub requires | comments naming what GitHub requires | ready for review |
| ready for review | GitHub blocks the merge, or has not reported whether it allows it | waits for what GitHub requires | ready for review |
| ready for review | otherwise | merges it ([Merging](#merging)) | merged |

- **Its checks.** The check runs and commit statuses on its head.
- **A round.** A round starts when Linewright opens the pull request, when a
  rework run on it ends and when Linewright is assigned to it again.
- **Ready to merge.** A PR is ready to merge when it is ready for review, its
  checks have passed, it has no feedback, no person holds its merge
  ([Merging](#merging)), it has everything the repository requires before a
  merge and, with `auto-merge` off, a person has approved it.

## CI

A fix-ci run fixes the checks that failed on the PR's head.

- **What a fix session is given.** For each failing check, its name, what it
  reports about the failure, the end of the output of the step that failed (at
  most its last 50 lines, without timestamps) and where to read its full log.
  Where the log does not show which step failed, the end of the whole log
  stands in for the step's output. A check with no log, such as a commit
  status, gives its description and its link instead.

## Rework

A comment or review from anyone other than Linewright is feedback until a
rework run that was given it ends done ([Ending](runs.md#ending)); a review
that only approves is not feedback.

- **The run.** It brings the branch up to date where it must be
  ([Keeping up to date](#keeping-up-to-date)), addresses the comments,
  pushes, replies in each thread and re-requests review from the people whose
  comments it addressed.
- **Resolving threads.** A reply that asks to resolve its thread resolves it
  when a bot opened the thread; people resolve their own threads.
- **Feedback during a run.** Feedback posted while a run works goes to its
  session before the run's pushes.
- **Its push.** Unlike Linewright's other writes
  ([Writes](repository.md#writes)), its pushes come before the replies, which
  describe them.

## Keeping up to date

A branch is brought up to date with the default branch where a row of
[Working a pull request](#working-a-pull-request) that starts an update run
holds, and by every fix-ci run.

- **Which runs.** An update run does only that. A rework run starts with it
  where the branch conflicts with the default branch or cannot be merged by
  its merge method, as those rows give. A fix-ci run always starts with it, so
  that its session can reproduce a failure the default branch's changes
  caused.
- **Merge or rebase.** An update rebases the branch onto the default branch
  where the merge method is merge or rebase, so the branch holds no merge
  commit, and merges the default branch into the branch where it is squash,
  so reviewed commits stay as they are.
- **Its own push.** A run that brings the branch up to date and then changes
  it further pushes, as it applies its result, the update on its own first
  and its other work right after, so that each push's comparison on GitHub
  shows either the default branch's changes or the run's, not both.

## Merging

With `--auto-merge`, Linewright merges the PRs it opened
([Working a pull request](#working-a-pull-request)); without it, the
maintainer merges them.

- **Merge method.** The repository's PRs are merged with the `merge-method`
  setting, whoever merges them.
- **The default.** By default the merge method is the first of merge, rebase
  and squash that the repository allows, merge only where the repository does
  not require linear history. A repository that allows none of them so
  fails the startup checks ([Startup](repository.md#startup)).
- **Linewright merging.** With `--auto-merge`, Linewright merges with the
  merge method. This is Linewright merging, not GitHub's auto-merge feature.
- **Review.** A person who has commented on or reviewed a PR holds its merge
  until their newest comment or review is an approval. Bots' comments don't
  hold a merge.
- **Which PRs.** Auto-merge covers every open PR Linewright opened, including
  those opened while it ran without `--auto-merge`.

## The sessions

A fix, rework or update session is sent `fix-ci.md`, `rework.md` or
`update.md`, with these placeholders filled besides those of every prompt
([Contract](runs.md#contract)).

| placeholder        | value                                                                                                                               |
| ------------------ | ----------------------------------------------------------------------------------------------------------------------------------- |
| `{{branch}}`       | the pull request's branch                                                                                                           |
| `{{pr}}`           | its number                                                                                                                          |
| `{{pr_title}}`     | its title                                                                                                                           |
| `{{description}}`  | its description                                                                                                                     |
| `{{ticket}}`       | the issue the pull request closes, as the implementer is given its ticket ([Implementation](implementation.md#the-session))       |
| `{{unfinished}}`   | the unfinished commits on the branch (`unfinished.md`), or nothing |
| `{{conflict}}`     | where the update stopped and the files in conflict (`update-rebase.md` or `update-merge.md`); for a fix or a rework, nothing unless it starts with an update that stopped (`conflict-rebase.md` or `conflict-merge.md`) |
| `{{failures}}`     | a fix's alone: each failing check, as under [CI](#ci), with how to read its full log (`full-log.md`) |
| `{{history}}`      | a fix's and a rework's: how to commit for the merge method ([Merging](#merging)): as a coherent series where it is merge or rebase (`history-series.md`), or on top of the branch's commits where it is squash (`history-on-top.md`) |
| `{{feedback}}`     | a rework's alone: the new comments, reviews and comments in threads                                                                 |
| `{{conversation}}` | a rework's alone: the pull request's comments and its unresolved threads                                                            |

The session ends with a result that matches
[`briefs/results/pull-request.schema.json`](../../briefs/results/pull-request.schema.json).
