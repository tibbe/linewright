---
name: implement
description: "Implement a piece of work based on a spec or set of tickets."
disable-model-invocation: true
---

<!-- Forked from mattpocock/skills, skills/engineering/implement (MIT, see
LICENSE). Made non-interactive for Linewright. -->

# Implement

Implement the work your task describes, on the branch that is checked out.
The ticket, its comments (a triage brief, earlier handbacks) and, for work on
an open pull request, the feedback to address are in your task. You may
read GitHub but must not write to it or push: Linewright pushes your commits,
opens or updates the pull request and posts what your result says.

1. Read the ticket and its comments. Read `GLOSSARY.md` if present, and the
   parts of the spec (the files matching the spec glob in your task) your
   work touches, not the whole spec. When your task holds the spec diff of
   the spec change the ticket was cut from, the files it changes are among
   those parts.
2. Before writing code, call the Skill tool with `linewright:tdd`, and work
   test-first as it says.
3. Run typechecking regularly, single test files regularly, and the
   repository's full checks (tests, linters, formatters, whatever its CI or
   docs say it runs) when the code is written, and fix what fails.
4. Review on every run, rework included: once done, call the Skill tool with
   `linewright:code-review` to review the work. Apply its findings (see
   _Review findings_), then run the repository's full checks again and fix
   what fails. Don't review a second time.
5. Commit your work to the current branch, with messages that say why. Leave
   no uncommitted changes and no rebase or merge in progress.

## The ticket's scope

Your task may name the ticket's parent issue and, when the ticket was cut from
a spec change, that spec change: pointers, not their content, though the spec
change's spec diff comes with them. The parent's other sub-issues are the
tickets cut alongside this one, and they own what this ticket leaves out; the
spec change and those tickets show what that is.
Read them on GitHub when the ticket's scope is in question, for example before
adding something the ticket doesn't ask for, and pass them to the code review
so its spec reviewer can too. You need not read them on every run.

## Review findings

Apply every finding, unless it is **wrong on the facts**: it misreads the code,
cites a rule or a ticket or spec line that doesn't say what it claims, or asks
for a change that would break something the ticket or spec requires. Preferring
your design, or judging the change not worth making, does not make a finding
wrong. For each finding you don't apply, say which and quote the evidence that
it is wrong. A finding the spec doesn't settle is not a spec gap unless it is a
product decision: a reasonable maintainer could want either answer, and the
answer changes what a user sees. Otherwise apply it like any other finding. A
finding that is a spec gap is not a fix.

## The spec is the maintainer's

Never change a file matching the spec glob, even to fill a gap; report the
gap instead.

The spec says what the software should do, not everything that follows from
it; how the code does it belongs in the code. So a question the spec leaves
open is a **spec gap** only when it is a product decision: a reasonable
maintainer could want either answer, the answer changes what a user sees, and
the spec's wording doesn't already settle it. Anything else is yours to
decide, as the spec and the code support best:

- the spec's wording already answers it, read for what it says;
- one answer is the obvious one, and no maintainer would pick the other;
- it is about how the code works rather than what a user sees, such as the
  order work is picked in or when a message is posted again.

For a spec gap:

- If the gap **blocks** the ticket, stop and answer with the `blocked` outcome
  and the question the maintainer must answer. Commit what you have first.
- If it doesn't block the ticket, choose the reading the spec and the code
  support best, carry on, and list the gap in `spec_gaps`: each becomes an
  issue for the maintainer describing the spec change needed. Skip gaps the
  task lists as already filed.

## When you cannot finish

If the work can't be done (the ticket contradicts the code, the checks can't
be made to pass, something outside the repository is needed), answer with the
`gave-up` outcome and the reason, in terms the maintainer can act on.
