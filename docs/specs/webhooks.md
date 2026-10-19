# Webhooks

By supporting webhooks, Linewright reacts to a change in the repository within
seconds of it happening: GitHub tells Linewright which issue or pull request
changed, and Linewright looks at it straight away. Webhooks are optional, and
Linewright works without them.

For example, when the maintainer comments on one of Linewright's pull
requests, the rework starts within seconds.

## User Stories

1. As an operator, I want Linewright to react to a change in the repository
   within seconds, so that work does not wait for the next poll.
2. As an operator, I want faster reactions without more calls to GitHub, so
   that Linewright stays within its rate limit.
3. As an operator, I want webhooks to be optional, so that Linewright still
   runs where GitHub cannot reach it.

## Turning webhooks on

Setting the webhook secret turns webhooks on.

- **The webhook.** The maintainer creates the repository's webhook by hand,
  in the repository's settings: creating one takes admin rights on the
  repository, which Linewright's account does not need and, with
  `--auto-merge`, must not have. Linewright cannot see the webhook, so a
  misconfigured one shows as deliveries that never arrive, or that arrive and
  are refused, which the webhook's Recent Deliveries on GitHub and
  Linewright's log both show.
- **The secret.** The webhook and Linewright share a secret, with which
  GitHub signs each delivery. The secret is a credential, so it comes from
  the environment only, as `LINEWRIGHT_WEBHOOK_SECRET`; Linewright does not
  accept deliveries without one. A delivery whose signature is missing or
  does not match is refused and logged, and its response does not say why.
- **Listening.** `serve` then also listens for deliveries on the
  `webhook-address` setting, a host and port (default `0.0.0.0:8080`), over
  plain HTTP; TLS, where wanted, is a reverse proxy's in front. `once`,
  `status` and the other commands do not listen.
- **Polling.** Polling continues at `poll-interval` with webhooks on: GitHub
  does not redeliver a failed delivery, so the poll is what catches a missed
  one.

## Deliveries

A delivery names what changed, and Linewright acts on that alone.

- **Acting on a delivery.** Linewright reads the issue or pull request again
  from GitHub and does what a poll would do with it, without a poll of
  everything. A push to the default branch names every open pull request
  Linewright opened. What it acts on is what it read, never the delivery's
  own account of the change, so a delivery that arrives late, twice or out
  of order changes nothing a poll would not.
- **Events.** The webhook sends the events about what a poll reads, and only
  those: issues, pull requests, their comments and reviews, CI and pushes.
  Linewright accepts any other event and ignores it.
- **Content types.** Linewright reads a delivery in either content type
  GitHub offers, `application/json` or `application/x-www-form-urlencoded`,
  as its `Content-Type` header says.
- **Refused deliveries.** A signed delivery that names no event, or one of an
  event Linewright acts on from which it cannot tell what it names, is
  refused and logged, and its response says why, so the webhook's Recent
  Deliveries on GitHub shows the reason.
- **Repeats.** A delivery seen before is not refused: a repeat costs a read of
  what it names, and the signature is what keeps others out.
