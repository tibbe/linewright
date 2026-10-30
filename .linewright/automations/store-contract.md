---
on:
  schedule: "0 5 * * *"
session-timeout: 30m
max-issues: 5
---
Check whether the local store and GitHub still agree on the store's contract
tests, and file an issue for each contract they no longer agree on.

1. Run the contract tests against the sandbox repository
   `tibbe/linewright-contract`:

   ```sh
   LINEWRIGHT_CONTRACT_REPO=tibbe/linewright-contract \
     cargo test --locked --test contract
   ```

2. When the tests cannot reach the sandbox repository, file one
   `ready-for-human` issue saying why, and nothing else.

3. Run each test that failed once more on its own, and file nothing for it
   when any of these holds:
   - It passes: it failed on GitHub's side, such as a rate limit.
   - An issue about it is open, whatever cause it names.

4. Otherwise file one `ready-for-agent` issue per test, titled "Make the
   local store agree with GitHub on CONTRACT", where CONTRACT says what the
   test checks. Quote its failure, and say to find which side changed: when
   GitHub did, change the test to match it, then the stores to pass it;
   otherwise fix the store that fails.
