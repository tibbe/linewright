# Repository

Linewright joins a repository the way a new teammate would: as a GitHub user
of its own, beside the people already working there, through the issues,
labels, assignees and pull requests they already use. It shares the
repository rather than owning it. GitHub is both how people steer it and
where they see what it is doing, so working with Linewright needs nothing but
GitHub, and a comment on the issue or pull request records every outcome.

For example, the maintainer hands an issue to Linewright by labeling it, and
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
  requests, and read and write issues and labels. Everything Linewright does
  on GitHub carries that identity, so the maintainer can review its PRs and
  see its claims as assignments. With `--auto-merge`, the account must not be
  able to bypass the default branch's rules (no admin rights, not on a
  ruleset's bypass list), or the reviews they require are skipped.
- **The sessions' token.** Optionally, a GitHub token for agent sessions
  (`LINEWRIGHT_SESSION_GITHUB_TOKEN`), which they are given in place of
  Linewright's, so that what a session may do on GitHub, such as only read,
  is the operator's to choose. Without one, sessions are given Linewright's
  token, and Linewright warns at startup that sessions can write to GitHub.
- **Spec files.** Tracked Markdown matching the spec glob (default
  `docs/**/*.md`).
- **Used when present.** The files the agents' briefs rely on,
  `CODING_STANDARDS.md` (what code review holds changes to) and `GLOSSARY.md`
  (the vocabulary agents read before exploring), and CI as checks on pull
  requests. The agent instructions and configuration the harness reads from
  the repository apply as in any session. The operator's own Claude Code
  setup does not, and sessions keep no memory of their own: what later
  sessions should know belongs in the repository.

## Labels

Linewright creates any of its labels that are missing at startup, except the
required label, and never renames or deletes one.

| label                                        | meaning                                           | set by                                           |
| -------------------------------------------- | ------------------------------------------------- | ------------------------------------------------ |
| `needs-triage`                               | not yet assessed                                  | reporter's issue template, maintainer, Linewright |
| `needs-info`                                 | its assessment waits on a fact from the reporter  | Linewright, maintainer                           |
| `ready-for-agent`                            | decided: an agent could build it                  | maintainer, Linewright                           |
| `ready-for-human`                            | needs a person's decision or action               | Linewright, maintainer                           |
| `bug`, `enhancement`                         | category, in repositories without issue types     | Linewright (triage), maintainer                  |
| `linewright:spec-processed`                  | this merged PR's spec changes have been ticketed  | Linewright                                       |
| the required label, named by `require-label` | Linewright may start work on this item            | maintainer, Linewright                           |

- **Prefix.** Labels only Linewright uses are prefixed `linewright:`.
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
  unassigned, marked for it, labeled for work it does, `needs-triage` for
  triage or `ready-for-agent` for implementation, and that it can start: no
  open issue blocks it and no open pull request closes it. An item Linewright
  was unassigned from, by itself or by anyone, is not claimed again on its
  own until its status label or the required label is applied again.
- **Assigned by hand.** Assigning Linewright to an item by hand asks it to
  work the item, marked for it or not and beside any other assignee. An issue
  it works as its label says, holding one it cannot start yet; one whose
  label asks nothing of it, it lets go of with a comment saying what the
  issue needs first. A pull request or a spec change it goes on with.
- **Cancelling.** A run is cancelled when its item stops asking for
  Linewright's work: Linewright is unassigned from the item, the item is
  closed, or an issue's label stops asking.
- **Handing back.** Handing an item back is how Linewright lets go of an item
  it cannot finish: it labels an issue `ready-for-human`, unassigns itself
  from the item and says why in a comment.
- **Marked for Linewright.** With `require-label` set, an item is marked for
  Linewright when it carries the required label, checked each time Linewright
  claims an item on its own. Work already started carries on without it.
  Linewright puts the label on every issue and pull request it creates.
  Automations run on their schedule either way. Unset, every issue and pull
  request is marked for Linewright.

## State

Machine state is derived from GitHub, never labeled.

- **In review.** An issue is in review while an open pull request closes it.
- **Linewright's pull requests.** They are the ones its account opened.

The lifecycle of an issue. `triage` and `work` each group several states,
given under [Triage](triage.md) and [Pull requests](pull_requests.md);
`ready-for-agent` and `ready-for-human` are the labels between them.

| State           | Event                          | Next            | Effect                                  |
| --------------- | ------------------------------ | --------------- | --------------------------------------- |
| —               | reporter files an issue        | triage          |                                         |
| —               | maintainer labels              | ready-for-agent |                                         |
| —               | Linewright creates a ticket    | ready-for-agent |                                         |
| triage          | clear and within the spec      | ready-for-agent |                                         |
| triage          | maintainer must decide         | ready-for-human |                                         |
| ready-for-human | maintainer labels needs-triage | triage          |                                         |
| ready-for-human | maintainer labels              | ready-for-agent |                                         |
| ready-for-agent | Linewright assigned            | work            |                                         |
| work            | Linewright unassigned          | ready-for-agent | not claimed on its own until labeled again |
| work            | handed back                    | ready-for-human |                                         |
| work            | PR closed unmerged             | ready-for-human |                                         |
| work            | PR merged                      | —               | issue closed                            |

## Comments

Linewright records every outcome in a comment on the item it concerns.

- **Which outcomes.** A triage result, tickets created (on the tracking issue
  and the spec PR), an item handed back and why, a PR made ready, a rework
  done. Claims, PRs and merges need no comment: the assignee, the PR and the
  merge are the record.
- **For people.** A comment is for people. Linewright decides nothing from
  what its own comments say: that it wrote one is the comment's author, and
  what it waits for is in the labels, assignees, pushes and pull requests.
- **Contents.** Comments contain only what belongs to the repository: no run
  ids, hosts or links to the factory.

## Startup

Linewright exits with an error at startup when it cannot work.

- **Checks.** The token can push to the repository; the sessions' token, when
  set, can read it; and, with `--auto-merge`, the repository allows the merge
  method and the update method keeps the branch mergeable with it
  (`update-branch` `merge` cannot keep a branch rebase-mergeable); with
  webhooks on, `serve` can listen on `webhook-address`; and `webhook-address`
  is not set without the webhook secret.
- **One Linewright per repository.** A `serve` started for a repository whose
  `serve` already runs on the same host exits with an error saying so.
