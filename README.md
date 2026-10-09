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
  that are due on their schedule. `status` lists the work in flight;
  `workflow run NAME` runs the workflow in `.linewright/workflows/NAME.md`
  once, now.
- Every setting is a flag, a `LINEWRIGHT_<FLAG>` environment variable or a key
  in `linewright.toml`; `linewright --help` lists them with their defaults.
- The `Dockerfile` builds the base image: `linewright`, `git` and Claude Code.
  Build your own image from it with your project's toolchain added.
