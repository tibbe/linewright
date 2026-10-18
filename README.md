# Linewright

Linewright is a spec-driven software factory: you write the specification, and
coding agents turn it into working software.

## Running it

You need `git` (2.45 or later), `bash` and
[Claude Code](https://code.claude.com) (`claude`) on the path, your project's
toolchain so agents can build and test it, and a GitHub user account for
Linewright with write access to the repository. Agents' commands run in `bash`,
whatever shell you use.

```sh
export GH_TOKEN=...            # that account's token (or GITHUB_TOKEN)
export LINEWRIGHT_SESSION_GITHUB_TOKEN=...  # optional: agents' token, see Secrets
export ANTHROPIC_API_KEY=...   # or any login Claude Code already has
cargo install --path .
lw --repo owner/name serve
```

Instead of `cargo install`, take `lw` from a
[release](https://github.com/tibbe/linewright/releases): Linux (`gnu`, or
static `musl` for Alpine and older distributions) on x86_64 and aarch64, and
macOS on Apple silicon.

- `lw serve` polls until stopped; `once` runs one poll cycle and the
  runs it starts; each poll also runs the workflows in `.linewright/workflows/`
  that are due on their schedule. `workflow run NAME` runs the workflow in
  `.linewright/workflows/NAME.md` once, now. One `serve` runs per repository
  on a host.
- `status` asks the running `serve` of the repository what it is doing: since
  when it has run, its polls, the usage limit while one is in force, how many
  runs are in flight, the deliveries with webhooks on, each run in flight with
  how long it has run and its quiet time, how long since its agent last produced
  output, the items it tracks, where each is and what Linewright waits for on
  it, and each workflow with its last and next run and any workflow file that
  does not parse. `explain N` asks it where issue or pull request #N stands,
  when it last read it, each thing Linewright waits for before it acts on it
  next, such as a free run, the usage limit, CI or the maintainer, its run in
  flight with its quiet time and its past runs as `runs N` lists them. For an
  item it does not track, `serve` reads #N from GitHub and `explain` says why
  Linewright leaves it alone, such as a missing label, a closed item, someone
  else's pull request or a merged one that is no spec change; it exits 4 when
  there is no #N. `--json` prints `serve`'s answer as JSON. Both exit 3 when no
  `serve` runs for the repository. `serve` answers on a Unix socket only its
  user can open, under `$XDG_RUNTIME_DIR/linewright/`, or `linewright-<uid>/` in
  the temporary directory without it.
- `runs [N]` lists the runs, newest first, or issue or pull request #N's:
  each run's ID (`43/rework-2`, the second rework run on pull request #43, or
  `weekly/3` for a workflow's), its item, stage, start, duration and outcome,
  including runs that started no session. `--stage`, `--since` and `--limit`
  narrow the list, and `--json` prints it as JSON. It asks the running
  `serve` or, with none running, reads the records `serve` keeps under the
  work directory, and exits 4 when #N has no runs.
- `logs N` shows the transcript of #N's run in flight or, with none, of its
  latest run that started a session; `--run ID` picks another run. Each line
  starts with the time since its session started: the agent's messages in
  full, each tool call on one line with a short account of its result, a
  sub-agent's work indented under the call that started it, and each turn
  after the first marked with the follow-up that started it. `--verbose`
  shows everything the transcript holds, also the agent's thinking, each
  tool's full input and result, and the brief and follow-ups. `--json`
  prints one JSON object per entry and line. `--follow` shows a run in
  flight's transcript so far, then each entry as it happens, a tool call as
  running once it is made and again once it returns, and exits when the run
  ends. Like `runs`, it asks the running `serve` or reads the records
  without one, and exits 4 when #N has no runs or the run ID names none.
  `serve` streams a followed transcript at `/logs/follow` as server-sent
  events, which a reader resumes by `Last-Event-ID`.
- Agent sessions ignore your user-level Claude Code setup: its settings,
  skills, plugins, MCP servers, `CLAUDE.md` and claude.ai connectors, and the
  `CLAUDE.md` files in directories above their clones. They read the
  repository's `AGENTS.md`, `CLAUDE.md`, `.claude/` and `.mcp.json`, plus any
  managed policy, and auto-memory is always off. Put what sessions need in the
  environment (variables named in `session-env`, see
  [Secrets](#secrets), and `ANTHROPIC_API_KEY` in place of an
  `apiKeyHelper`) or the repository (its `.claude/settings.json` or
  `.mcp.json`).
- Webhooks are optional; see [Webhooks](#webhooks).
- Every setting is a flag, a `LINEWRIGHT_<FLAG>` environment variable or a key
  in `linewright.toml`; `lw --help` lists them with their defaults.
- The `Dockerfile` builds the base image: `lw`, Claude Code,
  `git`, `gh`, and `python3` on Debian, without Node.js. Build your own image
  from it with your project's toolchain added. It is published for
  `linux/amd64` and `linux/arm64` under one tag, which pulls the host's
  architecture; if your image downloads tools built for one architecture,
  pick the download by the `TARGETARCH` build argument, as
  `deploy/Dockerfile` does for sccache. It runs as the user
  `linewright`, uid and gid 1000; to give a work directory on the host another
  owner, build it with the `LINEWRIGHT_UID` and `LINEWRIGHT_GID` build
  arguments. `deploy/` runs Linewright on itself in a locked-down container.

## Secrets

Agent sessions get only the environment variables they need, and Linewright
redacts its credentials from transcripts, its log and what it posts.

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
(`ANTHROPIC_*`, `CLAUDE_*`). Name the rest in `session-env`; a name ending in
`*` names every variable with that prefix. At startup Linewright logs the names
of the variables it keeps from sessions, so a build that fails only in a
session shows there what to add.

| for | add to `session-env` |
|---|---|
| Rust | `CARGO_HOME`, `RUSTUP_HOME` |
| Java | `JAVA_HOME` |
| Claude Code on Amazon Bedrock | `AWS_*` |
| Claude Code on Google Vertex AI | `CLOUD_ML_REGION`, `GOOGLE_APPLICATION_CREDENTIALS` |

For a Rust project, `linewright.toml` would have:

```toml
session-env = ["CARGO_HOME", "RUSTUP_HOME"]
```

### Credentials outside Linewright

Sessions can read Claude Code's credential and every file of the user
Linewright runs as. Give Linewright a Claude Code credential of its own, and
run it as a user, or in a container, without other credentials such as
`~/.config/gh`, `~/.netrc`, `~/.aws` or `~/.git-credentials`.

## Webhooks

With webhooks on, GitHub sends `serve` a delivery for each change in the
repository, and `serve` checks each one's signature, logs what it names, reads
that again and acts on it within seconds: an issue or pull request,
those of Linewright's open pull requests whose head commit a check or status
reports on, and, for a push to the default branch, all of them.
The poll still runs at `poll-interval` with webhooks on, for the reason
[the spec](docs/specs/webhooks.md) gives.

1. Choose a secret for the webhook to sign its deliveries with, such as one
   from `openssl rand -hex 32`.

2. Set `LINEWRIGHT_WEBHOOK_SECRET` to it where `serve` runs, and restart it.
   The secret comes from the environment only, never a flag or
   `linewright.toml`. `serve` then also listens over plain HTTP on
   `webhook-address`, `0.0.0.0:8080` unless set otherwise, and logs
   `listening for webhook deliveries` with the address.

3. Put a reverse proxy in front for TLS, such as Caddy or nginx, forwarding
   HTTPS to `webhook-address`; `deploy/README.md` does so with Nginx Proxy
   Manager. `serve` answers on any path.

4. Create the webhook by hand. Linewright cannot: creating one takes admin
   rights on the repository, which its account does not need and,
   with `--auto-merge`, must not have. As a repository admin, open the
   repository's **Settings → Webhooks → Add webhook** and set:
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

Linewright cannot see the webhook, so a misconfigured one shows only in the
log and in the webhook's **Recent Deliveries** tab on GitHub, which lists each
delivery with the answer it got and can redeliver it:

- **Deliveries never show up in the log.** If GitHub lists them with a
  connection failure, a timeout or an error from the reverse proxy, GitHub
  cannot reach `serve`: check the payload URL, the reverse proxy and
  `webhook-address`. If GitHub lists none, the webhook is not active or does
  not send that event.
- **GitHub lists deliveries as refused**, answered `401`. The log has
  `refused`, with why: with no signature, the webhook has no secret; with a
  signature that does not match, the webhook's secret and
  `LINEWRIGHT_WEBHOOK_SECRET` differ.
- **GitHub lists deliveries as refused**, answered `400` with a response that
  says why, which the log has too, as `refused`. With a content type that is
  neither `application/json` nor `application/x-www-form-urlencoded`,
  something between GitHub and `serve`, such as the reverse proxy, changed
  it.
