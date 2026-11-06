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

Every issue is designed before it is built; a ticket is created designed,
with the design of the issue it was cut from. How a design run ends, and
where it leaves the issue, is under
[Working an item](repository.md#working-an-item).

1. An agent designs the issue from what it asks: the issue and its comments and,
   for a spec change's issue ([Spec changes](sources_of_work.md#spec-changes)),
   the spec change and its spec diff.
2. It cuts the work into tickets where it needs cutting, as under
   [Cutting](#cutting).
3. Linewright records the design in a comment on the issue: the design and,
   where the work was cut, the tickets in order and why they are cut that
   way.

- **Spec gaps.** A spec gap that does not block the design is filed as under
  [Runs](runs.md).
- **Discarding a cut.** Discarding a cut closes the pull requests Linewright
  opened for its tickets and closes its open tickets as not planned, each
  with a comment, then
  removes its tickets from the issue's sub-issues. The issue's next design
  reads what the cut left.
- **Responsibility.** The design is Linewright's: it runs from a spec change
  or a claimed issue to the ready PRs with no human step, and a wrong design
  or set of tickets from a good spec is a defect in Linewright.

## Cutting

The design cuts work into tickets where it needs cutting, in the order they
are to be built.

- **Tickets.** Each ticket is a sub-issue of its tracking issue, carries
  `ready-for-agent` and `linewright:designed`, and is linked to the tickets
  it is blocked by with GitHub's issue dependencies.
- **The tracking issue.** An issue cut into tickets is their tracking issue
  while it carries `linewright:designed` and has tickets cut from it among
  its sub-issues: those Linewright created and added to it.

## The session

A design session is sent `design.md`, with these placeholders filled besides
those of every prompt ([Contract](runs.md#contract)).

| placeholder      | value                                                                                                                       |
| ---------------- | --------------------------------------------------------------------------------------------------------------------------- |
| `{{kind}}`       | `spec change` for a spec change's issue, and `issue` otherwise                                                              |
| `{{item}}`       | the issue and its comments, and for a spec change's issue the spec change's number, title, author, body and spec diff (`spec-change.md`) |
| `{{filed_gaps}}` | the spec gaps already filed for the issue (`filed-gaps.md`), or nothing |
| `{{cutting}}`    | how the work is cut (`cut-issue.md`, [Cutting](#cutting)) |

The session ends with a result that matches
[`briefs/results/design-issue.schema.json`](../../briefs/results/design-issue.schema.json).
