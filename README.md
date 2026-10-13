# Linewright

Linewright is a spec-driven software factory: you write the specification, and
coding agents turn it into working software.

## Running it

You need `git`, `bash` and [Claude Code](https://code.claude.com) (`claude`)
on the path, your project's toolchain so agents can build and test it, and a
GitHub user for the factory with write access to the repository. Agents'
commands run in `bash`, whatever shell you use.

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
- Webhooks are optional; see [Webhooks](#webhooks).
- Every setting is a flag, a `LINEWRIGHT_<FLAG>` environment variable or a key
  in `linewright.toml`; `linewright --help` lists them with their defaults.
- The `Dockerfile` builds the base image: `linewright`, `git`, and Claude
  Code on Debian, without Node.js. Build your own image from it with your
  project's toolchain added. It runs as the user `linewright`, uid and gid
  1000; to give a work directory on the host another owner, build it with the
  `LINEWRIGHT_UID` and `LINEWRIGHT_GID` build arguments. `deploy/` runs
  Linewright on itself in a locked-down container.

## Webhooks

With webhooks on, GitHub sends `serve` a delivery for each change in the
repository, and `serve` checks each one's signature, logs what it names and,
for an issue or a pull request, reads it again and acts on it within
seconds. A check, status or push delivery is only logged for now: the next
poll acts on it.
The poll still runs at `poll-interval` with webhooks on, for the reason
[the spec](docs/specs/product.md#webhooks) gives.

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

4. Create the webhook by hand. The machine user cannot: creating one takes
   admin rights on the repository, which the machine user does not need and,
   with `--auto-merge`, must not have. As a repository admin, open the
   repository's **Settings → Webhooks → Add webhook** and set:
   - **Payload URL**: the reverse proxy's HTTPS URL, such as
     `https://linewright.example.com/`.
   - **Content type**: `application/json`. A delivery sent form-encoded is
     logged as ignored, as its payload cannot be read.
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
- **The log has `ignored`, with a reason** that the payload is not JSON: set
  the webhook's content type to `application/json`.
