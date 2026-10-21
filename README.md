# Linewright

Linewright is a spec-driven software factory: you write the specification, and
coding agents turn it into working software.

Merge a change to the spec in your repository, or file an issue, and
[Claude Code](https://code.claude.com) agents implement it and open a pull
request. You steer it with issues, labels, comments and reviews.

## How it works

- **Spec in, pull request out.** A merged spec change or an issue starts the
  work, and a reviewed, green pull request ends it.
- **Agents check each other.** Separate sessions triage issues, design and
  implement the work, review it and fix failing CI before a person sees the
  pull request.
- **Everything happens on GitHub.** Linewright works as a GitHub user of its
  own. When it can't go on, it hands the item back with a comment saying why.
- **Recurring work.** Automations in `.linewright/automations/` run an agent on
  a schedule, such as a weekly sweep for bugs fixed without a regression test.

## Requirements

- `git` 2.45 or later, `bash` and Claude Code (`claude`) on the path
- your project's toolchain, so agents can build and test it
- a GitHub account for Linewright with write access to the repository, and its
  token
- an Anthropic API key, or any login Claude Code already has

## Quick start

Download `lw` from the
[latest release](https://github.com/tibbe/linewright/releases/latest), or build
it from source with `cargo install --path .`. Then:

```sh
export GH_TOKEN=...            # the token of Linewright's GitHub account
export ANTHROPIC_API_KEY=...
lw --repo owner/name serve
```

To run it in a container, build your own image from
`ghcr.io/tibbe/linewright`, adding your project's toolchain. See
[Running Linewright](docs/running.md#the-container-image).

Then file an issue: Linewright triages it and, once it is clear what to
build, builds it. The [user guide](docs/user-guide.md) shows how to steer it
with labels.

> [!WARNING]
> Agent sessions run commands and can read every file of the user Linewright
> runs as. Run it as its own user, or in a container, without other
> credentials, and give sessions a read-only GitHub token where you can. See
> [Secrets](docs/running.md#secrets).

## Documentation

- [Working with Linewright](docs/user-guide.md): filing, deciding and reviewing
  work through labels, comments and pull requests.
- [Running Linewright](docs/running.md): installation, configuration, GitHub
  setup and monitoring.
- `lw --help`: every command and setting.
- [`deploy/`](deploy/README.md): how Linewright runs on itself, locked down
  in Docker.

## License

[MIT](LICENSE)
