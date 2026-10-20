// Linewright's own script for the web UI (docs/specs/design_system.md): it
// says since when the page lost `serve`, shows times in the reader's time
// zone and durations in the reader's locale, keeping them up, and keeps a
// run's transcript at its latest entry while the reader is there. htmx's
// `hx-sse` extension keeps the page up from its event stream:
// https://four.htmx.org/extensions/hx-sse (htmx 4.0.0).
(() => {
  "use strict";

  const relative = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" });
  const absolute = new Intl.DateTimeFormat(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    timeZoneName: "short",
  });
  const units = [
    ["year", 31557600],
    ["month", 2630016],
    ["week", 604800],
    ["day", 86400],
    ["hour", 3600],
    ["minute", 60],
    ["second", 1],
  ];

  function ago(seconds) {
    for (const [unit, size] of units) {
      if (seconds >= size || unit === "second") {
        return relative.format(-Math.floor(seconds / size), unit);
      }
    }
  }

  // A duration in the reader's locale (`1h 5m 3s`), balanced into days,
  // hours, minutes and seconds, zero units left out. Where the browser lacks
  // Intl.DurationFormat, a duration keeps the server's text.
  const durations = Intl.DurationFormat && new Intl.DurationFormat(undefined, { style: "narrow" });
  // `durations` writes nothing for a duration of zero, which reads `0s`.
  const zeroDuration = Intl.DurationFormat &&
    new Intl.DurationFormat(undefined, { style: "narrow", secondsDisplay: "always" });
  // A settled duration, as the server writes one: `datetime="PT{seconds}S"`.
  const settledDuration = '[datetime^="PT"]';

  function duration(seconds) {
    if (seconds === 0) return zeroDuration.format({ seconds: 0 });
    return durations.format({
      days: Math.floor(seconds / 86400),
      hours: Math.floor((seconds % 86400) / 3600),
      minutes: Math.floor((seconds % 3600) / 60),
      seconds: seconds % 60,
    });
  }

  // A past time relative, with the absolute time on hover; a future one
  // absolute; a settled duration from its seconds, and a running one as long
  // as it has run.
  function tick() {
    const now = Date.now();
    for (const time of document.querySelectorAll(`time[datetime]:not(${settledDuration})`)) {
      const at = Date.parse(time.dateTime);
      if (Number.isNaN(at)) continue;
      const shown = absolute.format(at);
      if (at <= now) {
        time.textContent = ago(Math.floor((now - at) / 1000));
        time.title = shown;
      } else {
        time.textContent = shown;
        time.removeAttribute("title");
      }
    }
    if (!durations) return;
    for (const time of document.querySelectorAll(`time${settledDuration}`)) {
      const settled = /^PT(\d+)S$/.exec(time.dateTime);
      if (settled) time.textContent = duration(Number(settled[1]));
    }
    for (const time of document.querySelectorAll("time[data-since]")) {
      const since = Date.parse(time.dataset.since);
      if (Number.isNaN(since)) continue;
      time.textContent = duration(Math.max(0, Math.floor((now - since) / 1000)));
    }
  }

  const lost = document.getElementById("lost");
  let lostSince = null;

  // The run page's transcript stays at the bottom until the reader scrolls
  // up, and then offers to jump to the latest entry.
  const latest = document.getElementById("latest");
  const atBottom = () =>
    window.innerHeight + window.scrollY >= document.documentElement.scrollHeight - 40;
  const followed = (document.body.getAttribute("hx-sse:connect") || "").includes("follow=");
  let stuck = followed && !location.hash;

  function toBottom() {
    window.scrollTo(0, document.documentElement.scrollHeight);
  }

  // `entries`: whether the message added to the transcript.
  function swapped(entries) {
    tick();
    if (!latest || !entries) return;
    if (stuck) toBottom();
    else latest.hidden = false;
  }

  if (latest) {
    window.addEventListener("scroll", () => {
      stuck = atBottom();
      if (stuck) latest.hidden = true;
    });
    latest.addEventListener("click", () => {
      stuck = true;
      latest.hidden = true;
      toBottom();
    });
  }

  // An entry linked to opens the folds that hold it.
  function reveal() {
    const target = location.hash && document.getElementById(location.hash.slice(1));
    if (!target) return;
    for (let fold = target.closest("details"); fold; fold = fold.parentElement.closest("details")) {
      fold.open = true;
    }
    target.scrollIntoView();
  }

  // A message whose event ID differs from the last one's added entries to
  // the transcript.
  let lastEntryId = "";
  // Whether the extension has taken over the page's event stream.
  let streaming = false;

  function lose() {
    if (lostSince !== null) return;
    lostSince = new Date();
    lost.querySelector("time").dateTime = lostSince.toISOString();
    lost.hidden = false;
    tick();
  }

  document.body.addEventListener("htmx:sse:error", lose);
  document.body.addEventListener("htmx:sse:after:connection", () => {
    streaming = true;
    lostSince = null;
    lost.hidden = true;
  });
  document.body.addEventListener("htmx:sse:after:message", (event) => {
    const id = event.detail.message.id;
    const entries = id !== "" && id !== lastEntryId;
    lastEntryId = id;
    swapped(entries);
  });
  // The extension reconnects only a stream it has taken over, so a page
  // whose first connection failed, such as one loaded from the cache while
  // `serve` was down, asks again itself.
  document.body.addEventListener("htmx:finally:request", (event) => {
    if (event.target !== document.body || streaming) return;
    lose();
    setTimeout(() => {
      htmx.ajax("GET", document.body.getAttribute("hx-sse:connect"), { source: document.body });
    }, 5000);
  });

  tick();
  setInterval(tick, 1000);
  reveal();
  window.addEventListener("hashchange", reveal);
  if (stuck) toBottom();
})();
