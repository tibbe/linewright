# Repository

Linewright joins a repository the way a new teammate would: as a GitHub user
of its own, beside the people already working there, through the issues,
labels, assignees and pull requests they already use. It shares the
repository rather than owning it. GitHub is both how people steer it and
where they see what it is doing, so working with Linewright needs nothing but
GitHub, and a comment on the issue or pull request records every outcome.

For example, a maintainer hands an issue to Linewright by labeling it, and
sees that Linewright has taken it on because the issue is assigned to
Linewright.
When Linewright cannot finish it, a comment on the issue says why.

## User Stories

1. As an operator, I want to point Linewright at a repository and have it create
   what it needs, so that setup is a token and a flag.
2. As an operator, I want the repository to stay usable by people at the same
   time, so that the factory shares the repository rather than owning it.
3. As an operator, I want Linewright to refuse to start when it cannot work, so
   that a misconfiguration is loud.
4. As an operator, I want to start work with a label or an assignment and
   stop it by taking either away, so that control needs nothing but GitHub.
5. As an operator, I want Linewright to start work only on the issues and pull
   requests I mark for it, so that I can adopt it a little at a time in a
   repository whose people already use its labels.
6. As an operator, I want every outcome recorded on the issue or PR it concerns,
   so that GitHub is the audit trail.

## Requirements

Linewright needs a GitHub repository and a GitHub user account of its own in
it.

- **Linewright's account.** Its token lets it push branches, open pull
  requests, read and write issues and labels, and attach files to what it
  posts ([Attachments](runs.md#attachments)). Everything Linewright does
  on GitHub carries that identity, so a maintainer can review its PRs and
  see its claims as assignments. With `--auto-merge`, the account must not be
  able to bypass the default branch's rules (no admin rights, not on a
  ruleset's bypass list), or the reviews they require are skipped.
- **The sessions' token.** Optional
  ([What a session has](sessions.md#what-a-session-has)).
- **Spec files.** Tracked Markdown matching the spec glob (default
  `docs/**/*.md`).
- **Used when present.** CI, as checks on pull requests. The agent instructions
  and configuration the harness reads from the repository apply as in any
  session. The operator's own Claude Code setup does not, and sessions keep no
  memory of their own: what later sessions should know belongs in the
  repository.

## Linewright's files

The repository holds, under `.linewright/` on its default branch, what it
tells Linewright: its automations in `automations/`
([Automations](automations.md)), its settings for the work in `config.toml`
([The repository's file](operation.md#the-repositorys-file)) and its prompts
in `prompts/` ([Prompts](operation.md#prompts)).

- **When they are read.** As a run starts, from the default branch as it is
  then, so that a change takes effect once merged and every run on a branch
  works from the same files whatever the branch holds; the automation files
  are read at each poll instead ([Automations](automations.md)).
- **A file with an error.** A file under `.linewright/` that Linewright
  cannot use holds the runs it could change. A file that is not one of
  Linewright's, hidden files aside, could be meant for any run, so it holds
  every one. The items wait, and `status` and `explain` name the file and
  the error, until the default branch fixes it. An automation file with an
  error is skipped instead ([Automations](automations.md)).

## Labels

Linewright creates any of its labels that are missing at startup, except the
required label. Every label is the repository's: Linewright leaves it as the
people there keep it, except that at every startup it sets each
`linewright:` label's description to its meaning below.

| label                                        | meaning                                                | asks for work by                      | set by                                            |
| -------------------------------------------- | ------------------------------------------------------ | ------------------------------------- | ------------------------------------------------- |
| `needs-triage`                               | not yet assessed                                       | Linewright: triage                    | reporter's issue template, maintainer, Linewright |
| `needs-info`                                 | its assessment waits on a fact only a person knows     | a person                              | Linewright, maintainer                            |
| `ready-for-agent`                            | decided: an agent could build it                       | Linewright: design and implementation | maintainer, Linewright                            |
| `ready-for-human`                            | needs a person's decision or action                    | a person                              | Linewright, maintainer                            |
| `bug`, `enhancement`                         | category, in repositories without issue types          |                                       | Linewright (triage), maintainer                   |
| `linewright:designed`                        | designed, and cut into tickets where it needed cutting |                                       | Linewright                                        |
| `linewright:stalled`                         | handed back in a stall                                 |                                       | Linewright                                        |
| the required label, named by `require-label` | Linewright may start work on this item                 |                                       | maintainer, Linewright                            |

- **Linewright's labels.** Labels only Linewright uses are prefixed
  `linewright:`. Each records what Linewright has done to an item, beside the
  status labels: Linewright sets and clears it, and a person may change it
  to steer ([Steering](#steering)).
- **Status labels.** The first four are the status labels: what an issue
  needs next, one at a time. Whoever sets one removes the others; an issue
  carrying more than one holds the newest, and Linewright removes the rest
  ([Working an item](#working-an-item)).
  Pull requests carry none: a pull request's draft, review and check state
  say what it needs.

## Assignment

Assignees say who is working on an item: Linewright claims an item by
assigning itself and lets go of it by unassigning itself.

- **Closed items.** Linewright leaves a closed item's labels and assignees
  as they are.
- **Handing back.** Handing an item back is how Linewright lets go of an item
  it cannot finish: it labels an issue `ready-for-human`, unassigns itself
  from the item, removes `linewright:designed` where the design is the item's
  own, and says why in a comment.
- **Stalls.** A hand-back is a stall when Linewright judged nothing:
  something it relies on failed it, and a person fixes what failed or has
  Linewright try again. A stall labels the item `linewright:stalled` as well,
  cleared as under [Working an item](#working-an-item). For example, a
  session that timed out, a push GitHub refused, checks still failing after
  the fix attempts and a merge that failed twice are stalls; a question the
  spec cannot decide, a design with nothing to build, a session that gave up
  with its reason and a pull request closed unmerged are not.
- **Marked for Linewright.** With `require-label` set, an item is marked for
  Linewright when it carries the required label, checked each time Linewright
  claims an item on its own. Work already started carries on without it.
  Linewright puts the label on every issue and pull request it creates.
  Automations run on their schedule either way. Unset, every issue and pull
  request is marked for Linewright.

## State

An item's state is the first row of its table below that holds, derived from
what GitHub shows and, while a run is in flight on the item, from the run.

| An issue that                                                                                                                         | State                                        |
| ------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------- |
| is closed, or has had a pull request that closes it merge into the default branch since it was last opened or given a status label | done                                         |
| has an open pull request that closes it                                                                                               | in review                                    |
| is a tracking issue ([Cutting](design.md#cutting))                                                                                    | tracking                                     |
| holds `needs-triage` and is assigned to Linewright                                                                                    | being triaged                                |
| holds `ready-for-agent`, is assigned to Linewright and is without `linewright:designed`                                              | being designed                               |
| holds `ready-for-agent` and is assigned to Linewright                                                                                 | being implemented                            |
| holds a status label                                                                                                                  | its status label's, such as `needs-info`     |
| has a run in flight                                                                                                                   | the one it was in when the run started       |
| otherwise                                                                                                                             | no state                                     |

| A pull request that       | State  |
| ------------------------- | ------ |
| is open                   | open   |
| is merged                 | merged |
| is closed without merging | closed |

## Working an item

Linewright changes an item's labels and assignees, and starts work on it,
only as this table gives: at each poll, the first row that holds for the
item. Its writes are made in their order ([Writes](#writes)).

| State | When | Linewright | State after |
| --- | --- | --- | --- |
| any | the issue has tickets cut from it among its sub-issues and is without `linewright:designed` | discards the cut ([Discarding a cut](design.md#designing)) | as [State](#state) derives |
| being designed, being implemented | a person has closed without merging a pull request Linewright opened that closes it, since the issue was last opened or given a status label | hands it back | `ready-for-human` |
| `needs-info`, `ready-for-human`, no state | Linewright is assigned to the issue | unassigns itself, with a comment saying what the issue needs first where a person assigned it | unchanged |
| `needs-triage`, `ready-for-agent` | a person Linewright asked ([Answers](triage.md#answers)) is assigned to it | unassigns them | unchanged |
| tracking | it holds a status label or Linewright's assignment | removes the status label, then unassigns itself | tracking |
| tracking | it has sub-issues and every one is closed | closes it | done |
| `needs-info` | a reply has come ([Answers](triage.md#answers)) | labels it `needs-triage` | `needs-triage` |
| `needs-triage`, `ready-for-agent` | Linewright may claim it | assigns itself | being triaged, being designed, or being implemented where it carries `linewright:designed` |
| being triaged | no run is in flight on it | starts a triage run ([Triage](triage.md)) | being triaged |
| being triaged | the triage run ends with `ready-for-agent` | labels it `ready-for-agent` | being designed, or being implemented where it carries `linewright:designed` |
| being triaged | the triage run ends with `needs-info` | labels it `needs-info`, then assigns the people asked whom GitHub lets it assign | `needs-info` |
| being triaged | the triage run ends with `ready-for-human` | labels it `ready-for-human` | `ready-for-human` |
| being designed | no run is in flight on it | starts a design run ([Design](design.md)) | being designed |
| being designed | the design run builds it as it is | labels it `linewright:designed` | being implemented |
| being designed | the design run cuts it into tickets ([Cutting](design.md#cutting)) | creates the tickets, then labels it `linewright:designed` | tracking |
| being designed | the design run finds nothing to build, in an issue Linewright created | closes it as completed | done |
| being designed | the design run finds nothing to build in any other issue, or cannot make a design | hands it back | `ready-for-human` |
| being implemented | no run is in flight on it | starts an implement run ([Implementation](implementation.md)) | being implemented |
| being implemented | the implement run builds it | opens a pull request that closes it | in review |
| being triaged, being designed, being implemented | its run cannot finish ([Ending](runs.md#ending)) | hands it back | `ready-for-human` |
| merged | a spec change Linewright may take up ([Spec changes](sources_of_work.md#spec-changes)) | opens its issue | merged |
| open | a pull request Linewright opened, that it may claim | assigns itself | open |
| open | a pull request Linewright opened and is assigned to, for which [Pull requests](pull_requests.md) asks for a fix-ci, rework or update run or a merge | starts that run, or merges it | open, or merged |
| open | a run on it cannot finish ([Ending](runs.md#ending)) | hands it back | open |
| any | the issue carries more than one status label | removes all but the newest | unchanged |
| any | it carries `linewright:stalled` and Linewright is assigned to it | removes `linewright:stalled` | unchanged |

- **Claiming.** On its own, Linewright claims an issue that is unassigned,
  marked for it and that it can start: no open issue blocks it. It claims a
  pull request it opened. An item Linewright was unassigned from, by itself or
  by anyone, is not claimed again on its own until its status label or the
  required label is applied again.
- **Blocked.** A run starts on an issue only once no open issue blocks it.
- **A run in flight.** While a run is in flight on an item, only the rows for
  how that run ends hold for it.

## Steering

A person steers an item through GitHub, and Linewright follows.

| State | A person | Linewright | State after |
| --- | --- | --- | --- |
| any | closes the issue | cancels a run on it | done |
| done | opens the issue again | | as [State](#state) derives |
| any | changes or removes the issue's status label | cancels a run the label no longer asks for ([Labels](#labels)) | as [State](#state) derives |
| any | unassigns Linewright | cancels a run on the item | as [State](#state) derives |
| any | assigns Linewright to an issue, marked for it or not and beside any other assignee | | as [State](#state) derives |
| any | removes `linewright:designed` | | as [State](#state) derives |
| open | merges or closes a pull request | cancels a run on it | merged, or closed |
| open | assigns Linewright to a pull request Linewright opened | goes on with it, addressing what came in meanwhile | open |
| merged | assigns Linewright to a spec change not yet taken up ([Spec changes](sources_of_work.md#spec-changes)) | opens its issue, then unassigns itself | merged |
| open, merged, closed | assigns Linewright to any other pull request it did not open | unassigns itself with a comment | unchanged |

- **Cancelling.** When a cancel takes effect is under [Runs](runs.md).

## Managed items

Linewright manages an item it has worked or that asks for its work, while the
item has a state ([State](#state)).

- **Worked.** GitHub shows Linewright opening the item, claiming it, applying
  a label to it or opening a pull request that closes it. A comment alone is
  not working it.
- **Asking for its work.** An open item Linewright is assigned to, or one
  marked for Linewright whose status label asks for its work.

## Writes

Linewright changes an item so that, stopped between any two of its writes,
the item is in a state the spec already decides, asking for the work again or
for nothing the change has not yet recorded.

- **Order.** The writes that change what an item asks for, such as
  `linewright:designed`, its status label, its assignment or a pull request
  that closes it, come last.
- **Among them.** `linewright:designed` comes first, then the status label,
  then the assignment; for example, a new status label is added before the
  old one is removed, and both before Linewright unassigns itself.
- **One writer.** One of Linewright's runs or actions writes an item at a
  time.
- **A person's change during a run.** While a run works an item, Linewright
  acts on a person's change to it when the run ends, except a change that
  ends the run ([Ending](runs.md#ending)) and one the run takes up itself,
  such as feedback during rework ([Rework](pull_requests.md#rework)).
- **Its own writes.** Linewright tells its own writes from a person's by
  their author.

## Comments

Linewright records every outcome in a comment on the item it concerns.

- **Which outcomes.** Every outcome of a run, except those GitHub shows
  itself: a claim, a pull request and a merge need no comment, since the
  assignee, the PR and the merge are the record.
- **For people.** A comment is for people. Linewright decides nothing from
  what its own comments say: what it waits for is in the labels, assignees,
  pushes and pull requests.
- **Contents.** Comments contain only what belongs to the repository: no run
  ids, hosts or links to the factory.

## Startup

Linewright exits with an error at startup when it cannot work.

- **Checks.** The token can push to the repository; the sessions' token, when
  set, can read it; with `--auto-merge`, the repository allows the merge
  method and the update method keeps the branch mergeable with it
  (`update-branch` `merge` cannot keep a branch rebase-mergeable); with
  webhooks on, `serve` can listen on `webhook-address`; every setting's
  value is one it takes, and every setting of a feature that is off is at
  its default ([Configuration](operation.md#configuration)); every file in
  the operator's layer of prompts is one of Linewright's and has its
  placeholders ([Prompts](operation.md#prompts)); and, unless `ui-address`
  is empty, `serve` can listen on it.
- **One Linewright per repository.** A `serve` started for a repository whose
  `serve` already runs on the same host exits with an error saying so.
