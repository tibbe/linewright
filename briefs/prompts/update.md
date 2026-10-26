# Your task

Bring the pull request's branch below up to date with the default branch. You may read GitHub but must not write to it or push: Linewright pushes your commits and posts what your result says.

{{facts}}
- Branch: `{{branch}}`, the head of pull request {{pr}} "{{pr_title}}".{{unfinished}}

{{conflict}}

### The pull request's description

{{description}}

{{ticket}}

## Steps

### 1. Resolve the conflicts

Resolve them keeping both sides' intent.

### 2. Run the checks

Run the repository's full checks (tests, linters, formatters, whatever its CI or docs say it runs) and fix what the update broke.

### 3. Finish the update

Finish it as your task says; don't abort it.

### 4. Output

{{output}}

## Spec gaps

The spec is the maintainer's: change no file of it. A question the spec leaves open is a spec gap only when it is a product decision: a reasonable maintainer could want either answer, the answer changes what a user sees, and the spec's wording doesn't already settle it. Anything else is yours to decide, as the spec and the code support best.

A gap that blocks the work is the question of a `blocked` result. For a gap that doesn't, choose the reading the spec and the code support best, carry on, and list it in `spec_gaps`.
