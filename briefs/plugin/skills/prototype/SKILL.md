---
name: prototype
description:
  "Build a throwaway prototype to answer a design question. Use when a claim a
  design choice turns on is about a timing, a behavior or a shape."
---

# Prototype

A prototype is **throwaway code that answers a question**. The question
decides the shape.

## Pick the question

Identify which question is being answered: a claim the choice turns on, which
would flip the choice if it were false. State it as a prediction, "if <the
claim> holds, then <running this> shows <that>", at the top of the prototype.
Then pick the cheapest source that can answer it: the repository's own code
run locally, then a local instance of what it talks to, then a shared
environment, development before production.

## Rules

1. **Throwaway from day one, and clearly marked as such.** Keep the prototype
   in a scratch directory of its own, outside the repository, and name it so a
   casual reader can see it's a prototype, not production.
2. **Trivial to run.** One command starts it, and no thinking is required to
   start it.
3. **No persistence by default.** State lives in memory. Persistence is the
   thing the prototype is _checking_, not something it should depend on. If
   the question explicitly involves a database, hit a scratch DB or a local
   file with a clear "PROTOTYPE, wipe me" name. A shared environment, the
   prototype only reads.
4. **Skip the polish.** No tests, no error handling beyond what makes the
   prototype _runnable_, no abstractions. The point is to learn something
   fast.
5. **Surface the state.** After every action, print the full relevant state,
   the measurement, the output or the error, so what changed is visible. When
   two options are compared, run both under the same conditions, several
   times.
6. **Capture it when done.** Capture the answer: the verdict and the question
   it settled, with the observation that settled it and what was run, against
   what and how many times. The scratch directory goes with the session; the
   verdict is what remains. A prototype that doesn't answer leaves the
   question open, with what was tried.
