# Linewright

Linewright is a spec-driven software factory: you write the specification, and
coding agents turn it into working software.

## Running it

You need `git` and [Claude Code](https://code.claude.com) (`claude`) on the
path, your project's toolchain so agents can build and test it, and a GitHub
user for the factory with write access to the repository.

```sh
export GH_TOKEN=...            # the machine user's token (or GITHUB_TOKEN)
export ANTHROPIC_API_KEY=...   # or any login Claude Code already has
cargo install --path .
linewright --repo owner/name serve
```

- `linewright serve` polls until stopped; `once` runs one poll cycle and the
  runs it starts; each poll also runs the workflows in `.linewright/workflows/`
  that are due on their schedule. `status` lists the work in flight, each
  workflow with its last and next run, and any workflow file that does not
  parse; `workflow run NAME` runs the workflow in
  `.linewright/workflows/NAME.md` once, now.
- Agent sessions ignore your user-level Claude Code setup: its settings,
  skills, plugins, MCP servers, `CLAUDE.md` and claude.ai connectors, and the
  `CLAUDE.md` files in directories above their clones. They read the
  repository's `AGENTS.md`, `CLAUDE.md`, `.claude/` and `.mcp.json`, plus any
  managed policy, and auto-memory is always off. Put what sessions need in the
  environment (variables, and `ANTHROPIC_API_KEY` in place of an
  `apiKeyHelper`) or the repository (its `.claude/settings.json` or
  `.mcp.json`).
- Every setting is a flag, a `LINEWRIGHT_<FLAG>` environment variable or a key
  in `linewright.toml`; `linewright --help` lists them with their defaults.
- The `Dockerfile` builds the base image: `linewright`, `git` and Claude Code.
  Build your own image from it with your project's toolchain added.
