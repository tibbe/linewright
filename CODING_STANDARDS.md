# Coding standards

## Correct by construction

- **Make illegal states unrepresentable**: shape your types so that only valid
  combinations of data can be constructed.
- **Parse, don't validate**: a check that returns nothing forces every later
  caller to re-handle a case that's already been ruled out, so have it return a
  narrower type carrying what it learned, and do it once at the system boundary.

## Functional core, imperative shell

Decisions live in pure functions; I/O, time, randomness and UI live in a thin
layer that calls them.

## Comments

These rules cover comments in every file, code and config alike.

A comment is one of these:

- **Contract**: a doc comment stating what an item promises its callers,
  including its errors and panics.
- **Outside constraint**: behavior forced by a dependency, platform, vendor or
  protocol we cannot change, with a link to its primary source instead of a
  copy of its content.
- **Directive**: a comment a tool reads, such as a shebang or
  `# shellcheck disable=`, with the reason for a suppression.
- **License notice**.

When unsure whether a comment is one of these, delete it.

A surprise in our own code is fixed in the code, with a name, a type or a test.

A comment never says what the code does, refers to the task, fix, change or
callers ("added for the Y flow", "now uses X"), marks a section or holds
commented-out code.

## Testing

### Where tests go

A test checks a promise the code makes to someone outside it, at the seam
where the code makes it, with expected values taken from that party, never
from the code. Everything else (helpers, private functions, one module's
internals) is covered through these tests and gets no test of its own.

A test checks each case at the narrowest of these seams that decides it. A
test through the system covers a path, such as a stage's outcome, once, and
checks only what no narrower seam can show: the wiring, the order of writes
and races. It leaves out every case a narrower test checks. Where only the
system reaches a decision, the decision moves into the functional core.

- **The user, through the system**: drive it as a user would (a labeled issue,
  a merged PR) against a fake of each external system (GitHub, the harness,
  the clock) and check what the user would see. The web UI is driven
  through its router, in-process.
- **The user, through the functional core**: give a decision function a
  state of the world and check the decisions it returns. Every case is cheap
  to reach there.
- **An external system, through its adapter**: what the adapter reads from or
  writes to the system, such as GitHub's GraphQL and REST JSON, on payloads
  taken from that system (recorded, or its documented examples), not written
  by hand. The fakes stand in for the adapters, so no other test runs them.
- **An internal library's callers, through its interface**: for a library used
  throughout the code, not a helper function.
- **A fake, against its adapter**: the local store passes the same tests as
  the GitHub store, so the fake stays faithful to what it stands in for.
  Against GitHub, a daily automation runs them and files an issue for each
  one that fails.

### Setting up

A test sets up its state through the back door, and runs only the behaviour it
checks through the front door.

- **The back door.** A test writes the state it starts from, such as a
  repository's history or a pull request Linewright opened, straight into the
  fakes.
- **The front door.** Where the back door writes what a stage would leave, a
  test runs the stage and checks that it leaves the same, field for field.
- **Real git.** Tests run real git, in the code under test and in the fakes.
- **Fake programs.** A test runs only programs that cargo builds or that are
  installed on the machine. It tells a fake what to do with data, such as a
  scenario file.

### Failing

A failing test says where it failed and what led there.

- **Its log.** Every test is `#[test_log::test]`, around `#[tokio::test]`
  where it is async. `scripts/check test` enforces it.
- **Its line.** A helper that can fail a test is `#[track_caller]`. An async
  helper is a sync function that takes `Location::caller()` and returns the
  future.

### Anti-patterns

- **Racing a passing state**: the test waits for a state the system passes
  through, such as an assignment a run then undoes, so it passes only when a
  check lands in that window. Wait for the state the system comes to rest in,
  or script what follows so the state lasts.

- **Tautological**: the assertion recomputes the expected value the way the
  code does (`expect(add(a, b)).toBe(a + b)`, a snapshot derived by hand the
  same way, a constant asserted equal to itself), so it passes by construction
  and can never disagree with the code. Expected values must come from an
  independent source of truth: a known-good literal, a worked example, the
  spec.

## Existing code

Code that breaks these standards sets no precedent. Don't follow it, and don't
cite it to justify new code that breaks them.
