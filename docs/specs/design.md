# Design

A design decides the structure of the code before any of it is written, so
that a change is built to a structure chosen for it rather than patched into
the one that is there.

For example, a bug report says that exporting a large report fails. The
design finds that the exporter holds the whole report in memory, a structure
that makes the failure likely for any large report, and chooses one that
streams it. The work is cut in two: a ticket that makes the exporter stream,
then a ticket that fixes the export, and the bug becomes the tracking issue
of both.

## User Stories

1. As an operator, I want the structure of the code chosen before any of it
   is written, so that a change is built the best way known rather than
   patched into whatever structure is there.
2. As an operator, I want a fix to remove the kind of problem and not only the
   instance, so that issues like it stop coming.
3. As an operator, I want work cut into tickets an agent session finishes
   within its smart zone, so that the work is done by agents at their
   sharpest; that each PR is small enough to review is a welcome side effect.
4. As an operator, I want an issue too big for one session cut like a spec
   change, so that size never sends it back to me.
5. As an operator, I want tickets ordered by dependency and worked in that
   order, so that each builds on merged work.
6. As an operator, I want one place that shows a spec change's progress, so that
   I know when it is done.

## Designing

Every item is designed before it is built, by its first run; a ticket is
created designed, with the design of the item it was cut from.

1. An agent reads what the item asks (a spec change's PR and spec diff, or an
   issue and its comments), the spec and the codebase, and produces a design,
   chosen against the alternatives and recorded with them.
2. It cuts the work into tickets, as under [Cutting](#cutting).
3. Linewright records the design in a comment on the tracking issue or the
   issue: the design and, where the work was cut, the tickets in order and
   why they are cut that way. A spec change's PR gets the tickets in order
   and why, with a link to the tracking issue.
4. The item is labeled `linewright:designed`. An item carrying the label is
   not designed again.

- **1a. Nothing to build.** The code already does what the item asks. A spec
  change is labeled `linewright:designed`, and its PR gets a comment saying
  why; an issue is handed back, as triage hands back an issue already
  implemented.
- **1b. The design cannot be made.** The item is handed back. A spec gap that
  does not block the design is filed as under [Runs](runs.md).
- **4a. Interrupted before the label is set.** The outcome is discarded, its
  tracking issue and tickets closed as not planned with a comment, and the
  item is designed again, reading what the attempt left.
- **Responsibility.** The design is Linewright's: it runs from a spec change
  or a claimed issue to the ready PRs with no human step, and a wrong design
  or set of tickets from a good spec is a defect in Linewright.

## Keeping or changing the structure

The design keeps the code's structure where it holds up and changes it where
it does not: where the change would add a special case to the structure, or
where the problem is one the structure makes likely, so that its kind would
recur.

- **How much.** A changed structure covers the part the change touches,
  judged against the whole spec rather than the item alone.

## Cutting

Work is cut into tickets when it does not fit one session's smart zone or
changes the code's structure; the structure change is the first ticket, the
behavior change after it.

- **Tickets.** Vertical slices, each sized to a session, with code that
  already matches the spec producing none. Each ticket is a sub-issue of its
  tracking issue, states what to build and its acceptance criteria, carries
  `ready-for-agent` and `linewright:designed`, and is linked to the tickets
  it is blocked by with GitHub's issue dependencies.
- **A spec change.** Its work is always tickets, one when it fits a session,
  under a tracking issue Linewright creates: "Spec change: <PR title>",
  linking the PR and listing the tickets. A spec change's tracking issue is
  the open issue Linewright created that references the pull request.
- **An issue.** One that fits a session is built as it is; one that is cut
  becomes its tickets' tracking issue.
- **The tracking issue** carries no status label.

## Out of scope

- Building a spec change without tickets, as an issue is built.
