# Replay

Linewright is built to be tuned. How well the factory works depends on the
briefs that instruct its agents, and those get better against real work:
when the factory handles a spec change poorly, the briefs can be changed and
the same change run again, quickly and without touching the repository, until
the result is right.

For example, a person unhappy with the pull requests the factory made for a
spec change has a coding agent change a brief, run the change again and see
what the factory makes of it now, until it does better.

## User Stories

1. As an operator, I want to capture a spec change the factory handled as a case
   on disk, so that I can run it again.
2. As an operator, I want to run a case through the whole pipeline on my machine
   with GitHub out of the loop, so that a change to a brief is tried in minutes.
3. As an operator, I want a replay's whole output as files, so that a coding
   agent can read it, judge it and change the briefs.
4. As an operator, I want to edit the briefs between runs without rebuilding
   Linewright, so that iterating is editing a file.

## Cases

A case is a directory holding the repository at a commit and the event: a
spec change (the PR's title, body and the commits it merged) or an issue (its
text and comments).

- **Capturing.** `lw capture <repo>#<N> DIR` writes one from GitHub; run on
  the factory's machine it includes the transcripts of the sessions that
  handled it.
- **Transcripts.** Linewright keeps every run's transcripts (see
  [Runs](runs.md)), so a case can be captured with its transcripts later.

## Replaying

`lw replay CASE --out DIR` runs the pipeline end to end against a local store
seeded from the case.

- **The loop.** The same reconcile loop as `serve`, run until nothing is left
  to do rather than on a timer, ending when every ticket's pull request is
  ready for review (with `--auto-merge`, merged) or the ticket or its pull
  request is handed back.
- **Merging.** The local store has no branch rules, so with `--auto-merge`
  every green pull request merges and the tickets it blocked run in turn.
- **CI.** The implementer's own run of the repository's checks stands in for
  CI.
- **The output.** The output directory holds everything the replay produced,
  as plain files a coding agent reads directly. Replay runs the whole
  pipeline; what a stage did is read from its transcript.
- **What agents see.** Agents see only the case: a session reads issues,
  comments and spec diffs from its brief and hands back tickets, comments and
  PR bodies in its result, and the manager alone writes to the store. In
  replay agents get no GitHub credentials, so a session reads no newer state
  than the case and cannot touch the repository. Automations, whose agents read
  GitHub, are not captured or replayed.

## Briefs

Briefs are files Linewright reads at startup from the `briefs` directory
setting, so a change needs no rebuild.
