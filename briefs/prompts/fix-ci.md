# Your task

Make the failing checks below pass on the pull request. You may read GitHub but must not write to it or push: Linewright pushes your commits and posts what your result says.

{{facts}}
- Branch: `{{branch}}`, the head of pull request {{pr}} "{{pr_title}}".{{unfinished}}

### Failing CI

These checks fail on the pull request's head.

{{failures}}

### The pull request's description

{{description}}

{{ticket}}

## Steps

{{conflict}}

### 1. Find the cause

Read what each check reports, and its full log where the excerpt doesn't show the cause. Reproduce the failure where you can: run the check's command, or the repository's full checks (tests, linters, formatters, whatever its CI or docs say it runs).

### 2. Fix it

Fix the cause. Keep to the pull request's work: a fix adds nothing the ticket doesn't ask for. Then run the repository's full checks and fix what fails.

### 3. Commit

Commit your work to the current branch, with messages that say why. Leave no uncommitted changes and no rebase or merge in progress.

### 4. Output

{{output}}

## Spec gaps

The spec is the maintainer's: change no file of it. A question the spec leaves open is a spec gap only when it is a product decision: a reasonable maintainer could want either answer, the answer changes what a user sees, and the spec's wording doesn't already settle it. Anything else is yours to decide, as the spec and the code support best.

A gap that blocks the work is the question of a `blocked` result. For a gap that doesn't, choose the reading the spec and the code support best, carry on, and list it in `spec_gaps`.
