# Running Linewright on itself

`compose.yaml` runs Linewright on this repository in Docker, locked down:
agents reach the network only through
[Smokescreen](https://github.com/stripe/smokescreen), a proxy that refuses
private addresses, so they get the public internet but not the host, the LAN,
or other containers. Tools that ignore `https_proxy`, or use SSH, cannot
connect.

CI builds its images from each commit on `main` and pushes them to GHCR, tagged
`main` and `sha-` with the commit's first seven characters; a host pulls them;
`compose.build.yaml` builds them. The agents run as uid 10001 in the group
`users` (100). Run the commands here from `deploy/` in a clone of this
repository on the host, which keeps the host's `.env` and
`compose.override.yaml`; from another machine, run them there over SSH.

1. Copy `.env.example` to `.env`, readable by its owner alone (`chmod 600`), and
   fill it in: `GH_TOKEN`, the token of the factory's GitHub user, optionally
   `LINEWRIGHT_SESSION_GITHUB_TOKEN`, a read-only token for the agents (see the
   [running guide](../docs/running.md#a-read-only-github-token-for-sessions)),
   either `ANTHROPIC_API_KEY` or a `CLAUDE_CODE_OAUTH_TOKEN` from
   `claude setup-token`, and, for webhooks, `LINEWRIGHT_WEBHOOK_SECRET` (see
   [Webhooks](#webhooks)).

2. Optionally, set what this host needs in `compose.override.yaml`, such as a
   directory of the host's for the work directory, in place of a volume, owned
   by the agents' user (`chown 10001:100`), and limits. If you work on
   another machine, name the host and the clone's directory in
   `CLAUDE.local.md` at the root of your checkout there, so coding agents
   deploy and watch it over SSH too.

   ```yaml
   services:
     linewright:
       volumes: ["/srv/linewright:/data"]
       mem_limit: 8g
       environment: {LINEWRIGHT_MAX_RUNS: 2}
   ```

3. Pull and start it, and follow its log:

   ```sh
   docker compose pull && docker compose up -d
   docker compose logs -f
   ```

## Webhooks

GitHub's deliveries reach Linewright through
[Nginx Proxy Manager](https://nginxproxymanager.com) (NPM), which terminates
TLS, and `webhook-relay`, a socat container that forwards to
`linewright:8080` and nowhere else. The relay is on both `fenced` and the
external Docker network `proxy`, where NPM reaches it as `linewright-webhook`;
`linewright` stays on `fenced` alone, so the agents cannot reach NPM.

The relay always runs, so `docker compose up` fails until the `proxy` network
exists, webhooks on or not. Create it with `docker network create proxy`, and
attach NPM's container to it, as an external network in NPM's own
`compose.yaml`.

1. Set `LINEWRIGHT_WEBHOOK_SECRET` in `.env`, as
   [Running Linewright](../docs/running.md#webhooks) says, and rerun step 3
   above.

2. In NPM, add a proxy host:
   - **Domain names**: the public name for the webhook, such as
     `linewright.example.com`.
   - **Scheme** `http`, **Forward hostname** `linewright-webhook`, **Forward
     port** `8080`.
   - Under **SSL**, a Let's Encrypt certificate, with **Force SSL** on.

3. Create the repository's webhook as
   [Running Linewright](../docs/running.md#webhooks) says, with that domain name
   in the payload URL, such as `https://linewright.example.com/`, and the secret
   from step 1.

## Deploying and watching it

From another machine, run each command in the host's clone:
`ssh HOST 'cd DIR/deploy && docker compose ...'`.

- Deploy another version: once CI has pushed its images, `git pull`, so
  `compose.yaml` matches them, and rerun step 3. For a commit other than the
  latest on `main`, check it out and set `LINEWRIGHT_TAG=sha-...`. Docker
  drains the old container before it starts the new one, so `up -d` returns
  once the runs in flight have finished, up to `stop_grace_period`; `lw status`
  shows them.
- The commit it runs, from its image's labels:

  ```sh
  docker inspect linewright-linewright-1 \
    -f '{{index .Config.Labels "org.opencontainers.image.revision"}}'
  ```
- The server's log, a JSON line for each decision and action:
  `docker compose logs -f linewright`.
- What Linewright is doing: `docker compose exec linewright lw status`.
- What Linewright did: `docker compose exec linewright lw runs`, or
  `runs N` for issue or pull request #N's runs. Each run's record, with the
  sessions it started, is under `/data/OWNER/NAME/runs/` in the container.
- What an agent did: `docker compose exec linewright lw logs N`
  shows the transcript of #N's run in flight or else its latest run that
  started a session, `--run ID` another run's, and `--verbose` all of it.
- Each agent session's record, under
  `/data/OWNER/NAME/sessions/` in the container. There is a directory for
  each issue (`issue-N`), spec PR (`pr-N`) and automation (`automation-NAME`),
  and in it one for each session, named by its start time (UTC) and kind:
  - `brief.md`: what the session was asked, and `followup-N.md` after it;
  - `output.jsonl`: its output, written as it runs;
  - `result.json`, `transcript.jsonl` and `stderr.log`,
    written when it ends.

  For example, the latest session for issue 3:

  ```sh
  docker compose exec linewright bash -c \
    'ls -td /data/tibbe/linewright/sessions/issue-3/* | head -1'
  ```

- What the proxy refused: `docker compose logs proxy | grep '"allow":false'`.
