// Linewright's own script for the web UI (docs/specs/design_system.md): it
// says since when the page lost `serve`, shows times and durations in the
// page's language and the reader's time zone, keeping them up, keeps a run's
// transcript at its latest entry while the reader is there, keeps each
// fold as the reader opened or closed it through updates, and hides a
// tooltip on Escape. htmx's
// `hx-sse` extension keeps the page up from its event stream:
// https://four.htmx.org/extensions/hx-sse (htmx 4.0.0).
(() => {
  "use strict";

  // An update morphs the page into the server's, but for whether a fold is
  // open, which is the reader's until they open the page again. The hook is
  // htmx's `htmx:before:morph:attr` event:
  // https://four.htmx.org/docs/extension-authoring-guide#event-hooks
  htmx.registerExtension("linewright-folds", {
    htmx_before_morph_attr: (element, detail) =>
      !(element.tagName === "DETAILS" && detail.attrName === "open"),
  });

  // Every time and duration reads in the page's language, as the words
  // around it do, and in the reader's time zone.
  const lang = document.documentElement.lang;
  // "1 day ago", never "yesterday": the days are whole days back, not
  // calendar days.
  const relative = new Intl.RelativeTimeFormat(lang, { numeric: "always" });
  const absolute = new Intl.DateTimeFormat(lang, {
    year: "numeric",
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    timeZoneName: "short",
  });
  const dateThisYear = new Intl.DateTimeFormat(lang, { month: "short", day: "numeric" });
  const dateWithYear = new Intl.DateTimeFormat(lang, {
    year: "numeric",
    month: "short",
    day: "numeric",
  });

  // A past time `at` as of `now` in words, rounded down: `just now`, then
  // minutes, hours or days up to 30 days, then its date, with its year where
  // that is not this year's.
  function ago(at, now) {
    const seconds = Math.floor((now - at) / 1000);
    if (seconds < 60) return "just now";
    if (seconds < 3600) return relative.format(-Math.floor(seconds / 60), "minute");
    if (seconds < 86400) return relative.format(-Math.floor(seconds / 3600), "hour");
    if (seconds <= 30 * 86400) return relative.format(-Math.floor(seconds / 86400), "day");
    const thisYear = new Date(at).getFullYear() === new Date(now).getFullYear();
    return `on ${(thisYear ? dateThisYear : dateWithYear).format(at)}`;
  }

  // A duration in narrow units, balanced into days, hours, minutes and
  // seconds, of which it shows the two largest that are not zero (`1d 5m`).
  // Where the browser lacks Intl.DurationFormat, a duration keeps the
  // server's text.
  const durations = Intl.DurationFormat && new Intl.DurationFormat(lang, { style: "narrow" });
  // `durations` writes nothing for a duration of zero, which reads `0s`.
  const zeroDuration = Intl.DurationFormat &&
    new Intl.DurationFormat(lang, { style: "narrow", secondsDisplay: "always" });
  // A settled duration, as the server writes one: `datetime="PT{seconds}S"`.
  const settledDuration = '[datetime^="PT"]';

  function duration(seconds) {
    if (seconds === 0) return zeroDuration.format({ seconds: 0 });
    const balanced = {
      days: Math.floor(seconds / 86400),
      hours: Math.floor((seconds % 86400) / 3600),
      minutes: Math.floor((seconds % 3600) / 60),
      seconds: seconds % 60,
    };
    const largest = Object.entries(balanced).filter(([, n]) => n > 0).slice(0, 2);
    return durations.format(Object.fromEntries(largest));
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
        time.textContent = ago(at, now);
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

  // Escape hides a daisyUI tooltip that shows, until the pointer leaves it
  // or focus moves, as WCAG 1.4.13 asks
  // (https://www.w3.org/WAI/WCAG22/Understanding/content-on-hover-or-focus).
  // A tooltip whose `data-tip` is empty shows nothing.
  const tooltips = [...document.querySelectorAll("[data-tip]")].map((tooltip) => {
    const tip = tooltip.dataset.tip;
    const show = () => {
      tooltip.dataset.tip = tip;
    };
    tooltip.addEventListener("pointerleave", show);
    tooltip.addEventListener("focusout", show);
    return tooltip;
  });
  document.addEventListener("keydown", (event) => {
    if (event.key !== "Escape") return;
    for (const tooltip of tooltips) {
      if (tooltip.matches(":hover, :has(:focus-visible)")) tooltip.dataset.tip = "";
    }
  });

  tick();
  setInterval(tick, 1000);
  reveal();
  window.addEventListener("hashchange", reveal);
  if (stuck) toBottom();
})();
