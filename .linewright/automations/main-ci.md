---
on:
  workflow_run:
    workflows: [CI]
    branches: [main]
    conclusions: [failure]
session-timeout: 20m
max-issues: 2
---
File an issue for each cause of this CI failure on `main`.

1. File nothing for a cause when an issue about the same test or error is
   open, whatever cause it names.

2. Otherwise file one `ready-for-agent` issue per cause, titled "Fix CI on
   main: WHAT FAILED", quoting the failure. Say in it:
   - Which commit since the last CI run on `main` that passed caused it. When
     none can have, say that the test is flaky, and ask to fix what makes it
     fail.
   - Where a closed issue already tried to fix it, which one, and that its
     fix did not hold.
   - Where the failed job does not run on pull requests, that the next CI run
     on `main` is the proof of the fix.
