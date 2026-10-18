# Implementation

Implementation turns the work Linewright is given into reviewed, committed
changes. The spec stays the maintainer's: an agent builds from it and, where
it falls short, asks rather than changes it.

For example, the maintainer merges a spec change that adds invoices. Agents
implement creating, sending and listing invoices, run the repository's checks
and review each other's work, and the result reaches the maintainer as pull
requests that have already been through review.

## User Stories

1. As an operator, I want a spec change cut into tickets an agent session
   finishes within its smart zone, so that the work is done by agents at their
   sharpest; that each PR is small enough to review is a welcome side effect.
2. As an operator, I want tickets ordered by dependency and worked in that
   order, so that each builds on merged work.
3. As an operator, I want one place that shows a spec change's progress, so that
   I know when it is done.
4. As an operator, I want each ticket implemented on its own branch from a fresh
   checkout, so that runs never interfere with each other or with my working
   copy.
5. As an operator, I want the work reviewed by agents and passing the
   repository's checks before a PR exists, so that the first version I see has
   been through review.
6. As an operator, I want the spec left as it is by agents, so that it stays
   mine.
7. As an operator, I want the implementer to read only the spec its ticket
   touches, so that it stays in its smart zone and its run costs less.

## Decisions

### Tickets

- For each spec change an agent reads the PR's spec diff, the spec and the
  codebase and proposes tickets: vertical slices sized to a session's smart
  zone, with code that already matches the spec producing none.
- One **tracking issue** per spec change with tickets: "Spec change: <PR
  title>", linking the PR and listing the tickets, with no status label. Each
  ticket is a sub-issue of it, states what to build and its acceptance
  criteria, carries `ready-for-agent`, and is linked to the tickets it is
  blocked by with GitHub's issue dependencies.
- The PR is labeled `linewright:spec-processed` once its tickets exist, also
  when there are none, in which case no tracking issue is created.
- A decomposition that cannot finish hands the spec change back (see
  [Repository](repository.md)).
- A decomposition interrupted part way (the tracking issue exists, the PR is
  not labeled) is discarded, its tracking issue and tickets closed as not
  planned with a comment, and the spec change is cut again.
- Linewright closes the tracking issue when all its sub-issues are closed.
- The decomposition is Linewright's responsibility: it runs from a merged
  spec change to the ready PRs with no human step, and a wrong set of tickets
  from a good spec is a defect in Linewright.

### Working a ticket

- A run on a ticket starts from the ticket's branch when it has one, and
  otherwise from the default branch on a new branch for the ticket. The run
  may discard the branch's work and start again from the default branch. Once
  its pull request is open, the work is the pull request's (see [Pull
  requests](pull_requests.md)).
- The implementer reads the ticket and its comments (the triage brief, earlier
  hand-backs). Its brief names the ticket's parent issue, and for a ticket cut
  from a spec change, the spec change: pointers, not their content. The
  implementer and its reviewers read them when they need to know what the other
  tickets cover, since those own what this ticket leaves out. It implements,
  runs the repository's checks, has the work reviewed once, in fresh contexts
  for both standards and spec, applies every finding it cannot show is wrong
  on the facts, and commits.
- Of the spec, the implementer reads the glossary and the parts its work
  touches, not the whole spec. For a ticket cut from a spec change, its brief
  also holds the spec change's spec diff, and the files the diff changes are
  among the parts the work touches.
- The implementer leaves spec files as they are, by its brief; a spec it
  cannot build from is a spec gap, below. Linewright does not check the
  branch for spec changes: they show in the PR's diff, and a maintainer who
  wants a guarantee requires review of spec files in GitHub.
- A spec gap that blocks the run (the spec cannot decide it) hands its item
  back with the question. A gap that does not block it
  becomes a separate `ready-for-human` issue describing the needed spec
  change, and the run continues. A gap already filed for the item and still
  open is not filed again.
- A hand-back of a ticket with no pull request names the ticket's branch, so
  the maintainer can see what was tried, and deleting the branch starts the
  ticket's next run from the default branch. When a ticket is closed with no
  pull request open for it, Linewright deletes its branch.

## Out of scope

- One session for a whole spec change. Tickets exist to keep each session in
  its smart zone; as that zone grows, a spec change may need no cutting.
