# Running Linewright

This guide is for whoever runs Linewright. The [user guide](user-guide.md) is
for everyone who works with it on GitHub.

## Installing

You need `git` (2.45 or later), `bash` and
[Claude Code](https://code.claude.com) (`claude`) on the path, your project's
toolchain so agents can build and test it, and a GitHub user account for
Linewright with write access to the repository. Agents' commands run in
`bash`, whatever shell you use.

Download `lw` from a [release](https://github.com/tibbe/linewright/releases):
Linux on x86_64 and aarch64, `gnu` or static `musl` (for Alpine and older
distributions), and macOS on Apple silicon. Or build it from source with
`cargo install --path .`.

```sh
export GH_TOKEN=...            # Linewright's account's token (or GITHUB_TOKEN)
export LINEWRIGHT_SESSION_GITHUB_TOKEN=...  # optional: agents' token, see Secrets
export ANTHROPIC_API_KEY=...   # or any login Claude Code already has
lw --repo owner/name serve
```

`lw serve` polls the repository until stopped, and runs the automations in
`.linewright/automations/` that are due. One `serve` runs per repository on a
host. `lw drain` stops it once the runs in flight have finished. `lw once` runs
one poll cycle and the runs it starts, and `lw automation run NAME` runs the
automation in `.linewright/automations/NAME.md` once, now.

## Settings

Every setting is a flag, a `LINEWRIGHT_<FLAG>` environment variable or a key
in `linewright.toml`, except `session-env`, which has no variable;
`lw --help` lists them with their defaults.

### Stage settings

`model`, `effort` (`low`, `medium`, `high`, `xhigh` or `max`) and
`session-timeout` are for the whole pipeline, and a stage's table in
`linewright.toml` can set its own for its sessions. Without a `model` or
`effort`, Claude Code chooses. A stage's `session-pass-env` and `session-env`
add their variables to the whole pipeline's for that stage's sessions only,
and a value the stage's `session-env` sets wins over the whole pipeline's.
The stages are `triage`, `design`, `implement`, `rework`, `fix-ci` and
`update`; their tables have no flags or variables.

```toml
model = "sonnet"
effort = "medium"

[stage.implement]
model = "opus"
effort = "high"
session-timeout = "2h"
session-pass-env = ["DEPLOY_KEY"]
```

A model or effort Linewright sets wins over `ANTHROPIC_MODEL`,
`CLAUDE_CODE_EFFORT_LEVEL` and the repository's Claude Code settings. An
automation's `model` and `effort` in its front matter win over the whole
pipeline's for its runs.

### Prompts

`prompts` names a directory of your own versions of the prompts Linewright sends
sessions, laid out as Linewright's own in
[`briefs/prompts/`](../briefs/prompts/), such as `implement.md` or
`follow-ups/parse-error.md`. Each file in it is taken in place of Linewright's,
and the rest stay Linewright's. Linewright logs at startup which files it takes
from the directory, and exits with an error naming every file in it that is not
one of its prompts, is not UTF-8 text, or whose placeholders are not the
prompt's: one missing, or one Linewright doesn't know. Hidden files, whose name
or directory's name starts with `.`, are skipped.
[Tuning the briefs](tuning-briefs.md) has how to try a change.

## The container image

The `Dockerfile` builds the base image, `ghcr.io/tibbe/linewright`: `lw`,
Claude Code, `git`, `gh`, `jq`, `python3` and `ripgrep` on Debian, without
Node.js. Build your own image from it with your project's toolchain added.

The image is published for `linux/amd64` and `linux/arm64` under one tag,
which pulls the host's architecture. If your image downloads tools built for
one architecture, pick the download by the `TARGETARCH` build argument, as
`deploy/Dockerfile` does for sccache.

It runs as the user `linewright`, uid and gid 1000. To give a work directory
on the host another owner, build it with the `LINEWRIGHT_UID` and
`LINEWRIGHT_GID` build arguments. [`deploy/`](../deploy/README.md) runs
Linewright on itself in a locked-down container.

## Agent sessions

Agent sessions ignore your user-level Claude Code setup: its settings,
skills, plugins, MCP servers, `CLAUDE.md` and claude.ai connectors, and the
`CLAUDE.md` files in directories above their clones. They read the
repository's `AGENTS.md`, `CLAUDE.md`, `.claude/` and `.mcp.json`, plus any
managed policy, and auto-memory is always off.

Put what sessions need in the repository (its `.claude/settings.json` or
`.mcp.json`) or in the environment: variables named in `session-pass-env` or
set in `session-env` (see
[Variables your build needs](#variables-your-build-needs)), and
`ANTHROPIC_API_KEY` in place of an `apiKeyHelper`.

## Secrets

Agent sessions get only the environment variables they need, and Linewright
redacts secrets from transcripts, its log and what it posts.

### A read-only GitHub token for sessions

Without `LINEWRIGHT_SESSION_GITHUB_TOKEN`, sessions use Linewright's own
token, which can write, and Linewright warns at startup. Where GitHub allows
it, set it to a read-only token:

| repository owned by | `LINEWRIGHT_SESSION_GITHUB_TOKEN` |
|---|---|
| an organization Linewright's account is a member of | a [fine-grained token](https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/managing-your-personal-access-tokens) from that account, for that repository, with Read-only access to Actions, Commit statuses, Contents, Issues and Pull requests |
| an organization Linewright's account is an outside collaborator of | a classic token with the `repo` scope from a second account that has the Read role on the repository |
| a personal account | leave it unset: GitHub offers no read-only token there |

A fine-grained token
[can't call the Checks API](https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/managing-your-personal-access-tokens),
so sessions holding one can't list a pull request's checks with `gh pr checks`.
They still read GitHub Actions' runs and logs and commit statuses. The failures
of the checks Linewright asks a session to fix are in its brief.

### Variables your build needs

Sessions get the basic variables of a shell (such as `PATH`, `HOME` and the
locale), the proxy and certificate variables, and Claude Code's own
`ANTHROPIC_*` and `CLAUDE_*`. Name the rest in `session-pass-env` to pass them
from Linewright's environment; a name ending in `*` names every variable with
that prefix. Claude Code's settings outside those prefixes, such as
`DISABLE_TELEMETRY`, are part of the rest: pass them too, or set them in a
[managed settings file](https://code.claude.com/docs/en/managed-settings),
which sessions read. At startup Linewright logs the names of the variables it
keeps from sessions, and which of those each stage's table gives its sessions,
so a build that fails only in a session shows there what to add.

| for | add to `session-pass-env` |
|---|---|
| Rust | `CARGO_HOME`, `RUSTUP_HOME` |
| Java | `JAVA_HOME` |
| Claude Code on Amazon Bedrock | `AWS_*`, and its secrets, such as `AWS_BEARER_TOKEN_BEDROCK` or `AWS_SECRET_ACCESS_KEY`, to `secrets` |
| Claude Code on Google Vertex AI | `CLOUD_ML_REGION`, `GOOGLE_APPLICATION_CREDENTIALS` |
| Claude Code's other settings | their names, such as `DISABLE_TELEMETRY` or `BASH_DEFAULT_TIMEOUT_MS` |

To give sessions a value Linewright's environment doesn't hold, set it in
`session-env`, or with `--session-env NAME=value` once per variable. Values are
strings, used as written, and win over a variable of the same name passed from
Linewright's environment. For a Rust project, `linewright.toml` would have:

```toml
session-pass-env = ["CARGO_HOME", "RUSTUP_HOME"]
session-env = { RUST_LOG = "info", CARGO_TERM_COLOR = "never" }
```

Credentials your agents need for other services, such as a cloud account or a
database, go in `session-pass-env` too, and every session gets them; keep them
out of `session-env`, which is not redacted. Name the variables that hold
secrets in `secrets` as well, so that Linewright redacts them; it redacts on
its own those sessions get without your naming them, such as Claude Code's
login, a credential in its `ANTHROPIC_CUSTOM_HEADERS`, or a proxy's password:

```toml
session-pass-env = ["AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY", "AWS_REGION"]
secrets = ["AWS_SECRET_ACCESS_KEY"]
```

### Credentials outside Linewright

Sessions can read Claude Code's credential and every file of the user
Linewright runs as. Give Linewright a Claude Code credential of its own, and
run it as a user, or in a container, that holds no credentials you don't mean
sessions to have, such as in `~/.config/gh`, `~/.netrc`, `~/.aws` or
`~/.git-credentials`.

## Monitoring

`lw status` shows what the running `serve` is doing and what holds it back.
`lw explain N` shows where issue or pull request #N stands and what Linewright
waits for before it acts on it next, or why it leaves the item alone. `lw runs`
lists the runs, and `lw logs N` shows a run's transcript, with `--follow` as
it happens. `lw help COMMAND` has each one's options; each takes `--json`.

The commands talk to `serve` over a Unix socket that only its user can open,
under `$XDG_RUNTIME_DIR/linewright/`, or `linewright-<uid>/` in the temporary
directory without it. `runs` and `logs` read the records `serve` keeps in the
work directory when no `serve` runs. They exit 3 when they need a `serve` and
none runs for the repository, and 4 when #N or the run ID names nothing.

## The web UI

`serve` also serves a web UI, on `ui-address`, a host and port,
`127.0.0.1:8081` unless set otherwise, and logs that address as it starts. It
exits at startup if it cannot listen there. Set `ui-address` to an empty value
to serve no web UI. The UI has no login of its own: whoever reaches the port
can read it.

It answers only requests whose `Host` is `localhost`, an IP address or one of
the names in `ui-hosts`, ignoring case, port and a trailing dot, so that a web
page elsewhere cannot read it under a name it controls. Every other request
gets `421 Misdirected Request`, and `serve` logs each with the `Host` it named.

- **On the same machine**, open `http://localhost:8081`.
- **From another machine**, forward a port to it over SSH,
  `ssh -N -L 8081:localhost:8081 HOST`, and open `http://localhost:8081`.
- **Behind a reverse proxy** that authenticates its users and terminates
  TLS, add the name the proxy serves it under to `ui-hosts`, unless the proxy
  sends `Host: localhost`:

  ```toml
  ui-hosts = ["linewright.example.com"]
  ```

  As a flag or variable, `ui-hosts` is comma-separated:
  `--ui-hosts linewright.example.com,linewright.internal`.

In a container, set `ui-address` to `0.0.0.0:8081`, so the UI answers outside
the container, and publish the port on the host's loopback, such as
`127.0.0.1:8081:8081`. [`deploy/`](../deploy/README.md#the-web-ui) does that.

## Webhooks

Webhooks are optional. Without them, `serve` sees a change at its next poll,
every `poll-interval`. With them on, GitHub sends `serve` a delivery for each
change in the repository, and `serve` acts on it within seconds. The poll
still runs, for the reason [the spec](specs/webhooks.md) gives.

1. Choose a secret for the webhook to sign its deliveries with, such as one
   from `openssl rand -hex 32`.

2. Set `LINEWRIGHT_WEBHOOK_SECRET` to it where `serve` runs, and restart it.
   The secret comes from the environment only, never a flag or
   `linewright.toml`. `serve` then also listens over plain HTTP on
   `webhook-address`, `0.0.0.0:8080` unless set otherwise, and logs
   `listening for webhook deliveries` with the address.

3. Put a reverse proxy in front for TLS, such as Caddy or nginx, forwarding
   HTTPS to `webhook-address`; [`deploy/`](../deploy/README.md#webhooks) does
   so with Nginx Proxy Manager. `serve` answers on any path.

4. As a repository admin, create the webhook in the repository's
   **Settings → Webhooks → Add webhook**. Linewright's account doesn't need
   admin rights, and with `--auto-merge` must not have them. Set:
   - **Payload URL**: the reverse proxy's HTTPS URL, such as
     `https://linewright.example.com/`.
   - **Content type**: `application/json` or GitHub's default,
     `application/x-www-form-urlencoded`. Linewright reads either.
   - **Secret**: the secret from step 1.
   - **SSL verification**: enabled.
   - **Which events would you like to trigger this webhook?**: _Let me select
     individual events_, and tick **Issues**, **Issue comments**,
     **Pull requests**, **Pull request reviews**, **Pull request review
     comments**, **Check runs**, **Check suites**, **Statuses** and
     **Pushes**. Linewright ignores any other event.
   - **Active**: ticked.

GitHub sends a `ping` delivery when the webhook is created. The log shows it
as `ignored: not a change Linewright acts on`, with the event `ping`, which
shows that the webhook reaches `serve` and the secrets match. Each later
delivery of a ticked event about an issue, a pull request or a branch shows as
`accepted`, with what it names, such as `issue 12`; a push of a tag shows as
ignored.

### Troubleshooting webhooks

A misconfigured webhook shows only in Linewright's log and in the webhook's
**Recent Deliveries** tab on GitHub, which lists each delivery with the answer
it got and can redeliver it:

- **Deliveries never show up in the log.** If GitHub lists them with a
  connection failure, a timeout or an error from the reverse proxy, GitHub
  cannot reach `serve`: check the payload URL, the reverse proxy and
  `webhook-address`. If GitHub lists none, the webhook is not active or does
  not send that event.
- **GitHub lists deliveries as refused, answered `401`.** The log has
  `refused`, with why: with no signature, the webhook has no secret; with a
  signature that does not match, the webhook's secret and
  `LINEWRIGHT_WEBHOOK_SECRET` differ.
- **GitHub lists deliveries as refused, answered `400`**, with a response that
  says why, which the log has too. With a content type that is neither
  `application/json` nor `application/x-www-form-urlencoded`, something
  between GitHub and `serve`, such as the reverse proxy, changed it.
