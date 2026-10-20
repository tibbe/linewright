# Implementation

Implementation turns the work Linewright is given into reviewed, committed
changes. The spec stays the maintainer's: an agent builds from it and, where
it falls short, asks rather than changes it.

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

The implementer implements the ticket, runs the repository's checks, has the
work reviewed once, in fresh contexts for both standards and spec, applies
every finding it cannot show is wrong on the facts, and commits.

- **The branch.** A run on a ticket starts from the ticket's branch when it
  has one, and otherwise from the default branch on a new branch for the
  ticket. The run may discard the branch's work and start again from the
  default branch. Once its pull request is open, the work is the pull
  request's (see [Pull requests](pull_requests.md)).
- **What the implementer reads.** The ticket and its comments (the triage brief,
  the design, earlier hand-backs) and, for a ticket, the comments of the item
  it was cut from, which hold its design ([Design](design.md)). Its brief
  names the ticket's parent issue, and for a ticket cut from a spec change,
  the spec change: pointers, not their content. The implementer and its
  reviewers read them when they need to know what the other tickets cover,
  since those own what this ticket leaves out.
- **What it reads of the spec.** The glossary and the parts its work touches,
  not the whole spec. For a ticket cut from a spec change, its brief also
  holds the spec change's spec diff, and the files the diff changes are among
  the parts the work touches.
- **The spec stays as it is.** The implementer leaves spec files as they are,
  by its brief; a spec it cannot build from is a spec gap ([Runs](runs.md)).
  Linewright
  does not check the branch for spec changes: they show in the PR's diff, and
  a maintainer who wants a guarantee requires review of spec files in GitHub.
- **Handing back.** A hand-back of a ticket with no pull request names the
  ticket's branch, so the maintainer can see what was tried, and deleting the
  branch starts the ticket's next run from the default branch. When a ticket
  is closed with no pull request open for it, Linewright deletes its branch.

