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
   spec (the files matching the spec glob in your task).
2. Before writing code, call the Skill tool with `linewright:tdd`, and work
   test-first as it says.
3. Run typechecking regularly, single test files regularly, and the
   repository's full checks (tests, linters, formatters, whatever its CI or
   docs say it runs) once at the end, and fix what fails.
4. Once done, call the Skill tool with `linewright:code-review` to review the
   work, address its findings, and review again when the fixes were
   substantial. How many rounds it takes is your judgment, bounded
   by the time you have.
5. Commit your work to the current branch, with messages that say why. Leave
   no uncommitted changes and no rebase or merge in progress.

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
