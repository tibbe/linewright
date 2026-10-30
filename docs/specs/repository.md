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
  to steer; for example, removing `linewright:designed` from an item whose
  design is its own has it designed again.
- **Status labels.** The first four are the status labels: what an issue
  needs next, one at a time. Whoever sets one removes the others; an issue
  carrying more than one holds the newest, and Linewright removes the rest.
  Pull requests carry none: a pull request's draft, review and check state
  say what it needs.

## Assignment

Assignees say who is working on an item: Linewright claims an item by
assigning itself and lets go of it by unassigning itself.

- **While assigned.** It is assigned to an open issue only while the issue's
  label asks for its work, and to a pull request or a spec change only while
  it works it. The people it assigns to an open issue, it unassigns when the
  issue's label asks for its work again. A closed item keeps its labels and
  assignees.
- **Claiming on its own.** On its own, Linewright claims an issue that is
  unassigned, marked for it, whose status label asks for its work, and that
  it can start: no open issue blocks it and no open pull request closes it.
  An item Linewright was unassigned from, by itself or by anyone, is not
  claimed again on its own until its status label or the required label is
  applied again.
- **Assigned by hand.** Assigning Linewright to an item by hand asks it to
  work the item, marked for it or not and beside any other assignee. An issue
  it works as its label says, holding one it cannot start yet; one whose
  label asks nothing of it, it lets go of with a comment saying what the
  issue needs first. A pull request or a spec change it goes on with.
- **Cancelling.** A run is cancelled when its item stops asking for the run:
  Linewright is unassigned from the item, the item is closed, or an issue's
  label stops asking for the run, such as `needs-triage` relabeled
  `ready-for-agent` during triage. When a cancel takes effect is under
  [Runs](runs.md).
- **Handing back.** Handing an item back is how Linewright lets go of an item
  it cannot finish: it labels an issue `ready-for-human`, unassigns itself
  from the item, removes `linewright:designed` where the design is the item's
  own, and says why in a comment.
- **Stalls.** A hand-back is a stall when Linewright judged nothing:
  something it relies on failed it, and a person fixes what failed or has
  Linewright try again. A stall labels the item `linewright:stalled` as well,
  cleared when Linewright next claims or works the item. For example, a
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

State GitHub already shows is derived from it, never labeled.

- **In review.** An issue is in review while an open pull request closes it.

The lifecycle of an issue. `triage` and `work` each group several states,
given under [Triage](triage.md) and [Pull requests](pull_requests.md);
`ready-for-agent` and `ready-for-human` are the labels between them, and
`tracking` is an issue cut into tickets, under [Design](design.md).

| State           | Event                                  | Next            | Effect                                     |
| --------------- | -------------------------------------- | --------------- | ------------------------------------------ |
| —               | reporter files an issue                | triage          |                                            |
| —               | maintainer labels                      | ready-for-agent |                                            |
| —               | Linewright creates a ticket            | ready-for-agent |                                            |
| triage          | clear and within the spec              | ready-for-agent |                                            |
| triage          | maintainer must decide                 | ready-for-human |                                            |
| triage          | status label removed                   | —               |                                            |
| ready-for-human | maintainer labels needs-triage         | triage          |                                            |
| ready-for-human | maintainer labels                      | ready-for-agent |                                            |
| ready-for-human | status label removed                   | —               |                                            |
| ready-for-agent | Linewright assigned                    | work            |                                            |
| ready-for-agent | status label removed                   | —               |                                            |
| work            | Linewright unassigned                  | ready-for-agent | not claimed on its own until labeled again |
| work            | status label removed, no PR closes it  | —               | Linewright lets go of it                   |
| work            | handed back                            | ready-for-human |                                            |
| work            | PR closed unmerged                     | ready-for-human |                                            |
| work            | cut into tickets                       | tracking        | tickets filed as sub-issues                |
| tracking        | every sub-issue closed                 | —               | issue closed                               |
| work            | PR merged                              | —               | issue closed                               |

## Managed items

Linewright manages the items it has worked or that ask for its work, except
an open issue that holds no status label, has no run in flight, has no open
pull request closing it and is not the tracking issue of an open ticket,
whatever Linewright did on it before, such as one whose status label a person
removed.

- **Worked.** Linewright has opened the item, or recorded an outcome of its
  work on it ([Comments](#comments)).
- **Asking for its work.** An open item Linewright is assigned to, or one
  marked for Linewright whose status label asks for its work; and a spec
  change Linewright takes up ([Spec changes](sources_of_work.md#spec-changes)),
  until it is designed.

## Writes

Linewright changes an item so that, stopped between any two of its writes,
the item is in a state the spec already decides, asking for the work again or
for nothing the change has not yet recorded.

- **Order.** The write that changes what an item asks for, such as its status
  label, its assignment or a pull request that closes it, comes last; for
  example, a new status label is added before the old one is removed.
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
