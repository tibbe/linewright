---
name: verify-web-ui
description: "Drive Linewright's web UI in a browser and capture proof of what it did. Use to verify a change before its PR, to reproduce a reported web UI bug, to see what a page actually renders, or to capture a screenshot or recording of its behaviour."
---

# Verify the web UI

You serve the web UI over fakes, drive it in a browser the way a user would,
and leave behind evidence a human can read without rerunning anything.

## The server is yours

```bash
scripts/serve-fake   # in the background; prints the web UI's URL, then a command
```

`serve-fake` serves the web UI as `lw serve` does, over a local store standing
in for GitHub and `fake-claude` standing in for Claude Code, both set up by its
seed, `src/bin/serve-fake/seed.rs`. Serve the web UI with it alone: `lw serve`
works the real repository. Its clock starts at 2026-03-02 09:00 UTC on every
run. The seed's runs move on as real ones do: its implement session is in
flight for about a minute and a half. Capture a state once the page shows it.

When the page your change touches needs a state the seed lacks, such as an
item in another lane or a transcript entry of another kind, add it to the
seed in your change.

## Drive it

Call the Skill tool with "playwright-cli". Before going to the URL, run the
`playwright-cli run-code` command `serve-fake` printed after it, so the browser
tells `serve-fake`'s time: the pages work out how long ago and how long for
in the browser.

## Evidence

What makes it a proof:

- **The real user path.** Reach the page the way a user does: from the
  board, by its links and toggles.
- **The action and its result.** Capture the page your action starts from
  and the page it leaves: a screenshot and the snapshot of each, named alike.
  The snapshot is what you drove by, and a reader can diff it.
- **This diff.** A screenshot before, on `origin/main`, and after, on your
  branch, by the same steps, so the pair shows what this diff changed. Take
  the before in a worktree of `origin/main` with your change to the seed
  brought over, so the same steps reach the same state. When the interaction
  is the change (a live update, a transition, a toggle), a short recording
  from the same starting point, instead.
- **The console.** It catches the error a screenshot renders straight
  through.

Name files after the step. Save each file in the attachments directory your
task names, and refer to it by its absolute path there.

## Cleanup

Close the browser session, then stop `serve-fake` with `kill -QUIT`, which
removes its directory. Do it after a failed attempt too.
