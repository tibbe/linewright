# Linewright

Linewright is spec-driven: the specs decide what it does, and code follows
them. In an interactive session, your work is the conversation: answer
questions, discuss designs and draft spec changes. Write code when the user
asks for code; a ticket you are implementing is that ask. When a request could
mean either, ask which they want.

It is written in Rust and built with cargo. It runs on Linux and macOS alike,
on a machine or in a container. `deploy/` is one deployment among them:
Linewright's own, in Docker on Linux.

## Specs

Write and change the specs with the `spec-writing` skill. Their supporting
material, the files they name by path such as the briefs in `briefs/`, is part
of what they decide, each in its own form rather than spec prose.

Keep the specs in step with user-visible behavior changes. Changes to specs
require human approval.

## Briefs

The briefs are what people write for Linewright's agents: the skills, the
prompts, Linewright's own wording of what it fills into them, and the result
schemas. All of it lives in `briefs/`. Edit it only in an interactive session.

## Coding standards

`CODING_STANDARDS.md` should always be read before any code review.

## Checks

- Every check CI runs: `scripts/check`
- Only the named checks (`scripts/check --help` lists them):
  `scripts/check NAME...`
- Fix what the checks report where a tool can, such as formatting and the
  generated files (`scripts/fix --help` lists them): `scripts/fix`, or
  `scripts/fix NAME...`
- The model's TLC checks run only when named, and are long: `model-quick`
  takes about 50 minutes and `model` over six hours. Run one only when the
  human asks for it, in the background. To check a model change, check only
  the properties it bears on, which takes minutes:
  `scripts/check model MODEL NAME...` (the `verify-spec` skill).

## Verifying a change in the web UI

Call the Skill tool with "verify-web-ui" to drive the web UI in a browser over
fakes; it captures what the pages did. Use it to reproduce a web UI bug, too.

**Any change with a visible effect in the web UI, in a ticket you are
implementing, is checked in the browser with "verify-web-ui" before the PR is
opened.** Passing tests and a green `scripts/check` do not replace it. The PR
body states what you clicked and what you saw, with before and after
screenshots, or a short recording when the interaction is the change. A change
with no UI effect says so in one line — `No UI change` — instead.

## Area guides

Before working in one of these areas, read its guide:

- [Commits](./docs/commits.md)
- [Tuning the briefs](./docs/tuning-briefs.md)
- [Releasing](./docs/releasing.md)
- [Deploying Linewright on this repository](./deploy/README.md): its image is
  what Linewright's own sessions run in, so a tool they need goes there

## User-facing documentation

- `README.md`: the front page: a short introduction, a quick start and links to
  the guides.
- `docs/user-guide.md`: how to work with Linewright on GitHub.
- `docs/running.md`: how to operate it: setup, configuration and monitoring.

Any user-facing documentation changes require human approval.
