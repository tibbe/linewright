# Implementation

Implementation turns the work Linewright is given into reviewed, committed
changes. The spec stays the maintainer's: an agent builds from it and, where
it falls short, asks.

For example, the maintainer merges a spec change that adds invoices. Agents
implement creating, sending and listing invoices, run the repository's checks
and review each other's work, and the result reaches the maintainer as pull
requests that have already been through review.

## User Stories

1. As an operator, I want each ticket implemented on its own branch from a fresh
   checkout, so that runs never interfere with each other or with my working
   copy.
2. As an operator, I want the work reviewed by agents and passing the
   repository's checks before a PR exists, so that the first version I see has
   been through review.
3. As an operator, I want the spec left as it is by agents, so that it stays
   mine.
4. As an operator, I want the implementer to read only the spec its ticket
   touches, so that it stays in its smart zone and its run costs less.

## Working a ticket

The implementer builds the ticket and commits the work on the ticket's
branch.

- **The branch.** A run on a ticket starts from the ticket's branch when it
  has one, and otherwise from the default branch on a new branch for the
  ticket. The run may discard the branch's work and start again from the
  default branch. Once its pull request is open, the work is the pull
  request's (see [Pull requests](pull_requests.md)).
- **The spec.** A spec the implementer cannot build from is a spec gap
  ([Runs](runs.md)). Linewright does not check the branch for spec changes: they
  show in the PR's diff, and a maintainer who wants a guarantee requires review
  of spec files in GitHub.
- **Handing back.** A hand-back of a ticket with no pull request names the
  ticket's branch, so the maintainer can see what was tried, and deleting the
  branch starts the ticket's next run from the default branch. When a ticket
  is closed with no pull request open for it, Linewright deletes its branch.

## The session

The implementer's session is sent `implement.md`, with these placeholders
filled besides those of every prompt ([Contract](runs.md#contract)).

| placeholder          | value                                                                                                                                                                                                                                               |
| -------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `{{default_branch}}` | the default branch                                                                                                                                                                                                                                  |
| `{{branch}}`         | the ticket's branch                                                                                                                                                                                                                                 |
| `{{holding}}`        | whether the branch is new or holds earlier work, and its unfinished commit                                                                                                                                                                          |
| `{{ticket}}`         | the ticket and its comments (the triage brief, the design, earlier hand-backs); its parent issue, if it has one, by number, and the parent's comments, which hold its design ([Design](design.md)); for a ticket cut from a spec change, the spec change, by number, and its spec diff; and the spec gaps already filed for it |

The session ends with a result that matches
[`results/implement.schema.json`](results/implement.schema.json).
