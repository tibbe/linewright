// Linewright's own script for the web UI (docs/specs/design_system.md): it
// keeps the page up from its event stream, says since when it lost `serve`,
// shows times in the reader's time zone, keeping them up, and keeps a run's
// transcript at its latest entry while the reader is there.
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

  // As the commands write a duration: humantime's format, to the second.
  function duration(seconds) {
    if (seconds <= 0) return "0s";
    const years = Math.floor(seconds / 31557600);
    const ydays = seconds % 31557600;
    const months = Math.floor(ydays / 2630016);
    const mdays = ydays % 2630016;
    const parts = [];
    const plural = (value, name) => {
      if (value > 0) parts.push(value + name + (value > 1 ? "s" : ""));
    };
    const single = (value, name) => {
      if (value > 0) parts.push(value + name);
    };
    plural(years, "year");
    plural(months, "month");
    plural(Math.floor(mdays / 86400), "day");
    single(Math.floor((mdays % 86400) / 3600), "h");
    single(Math.floor((mdays % 3600) / 60), "m");
    single(mdays % 60, "s");
    return parts.join(" ");
  }

  // A past time relative, with the absolute time on hover; a future one
  // absolute; a running duration as long as it has run.
  function tick() {
    const now = Date.now();
    for (const time of document.querySelectorAll("time[datetime]")) {
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
  const followed = (document.body.dataset.events || "").includes("follow=");
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

  // Each message applied once the one before it has been, as a message can
  // add to what the one before added.
  let swapping = Promise.resolve();
  // The ID of the last transcript line taken, which a stream opened anew
  // follows on from, as one that reconnects itself does.
  let lastId = null;

  function connect() {
    const events = new URL(document.body.dataset.events, location.href);
    if (lastId !== null && events.searchParams.has("follow")) {
      events.searchParams.set("follow", lastId);
    }
    const source = new EventSource(events);
    source.onopen = () => {
      lostSince = null;
      lost.hidden = true;
    };
    source.onmessage = (event) => {
      const entries = event.lastEventId !== "" && event.lastEventId !== lastId;
      if (entries) lastId = event.lastEventId;
      swapping = swapping
        .then(() => htmx.swap({ text: event.data, sourceElement: document.body }))
        .then(() => swapped(entries), console.error);
    };
    source.onerror = () => {
      if (lostSince === null) {
        lostSince = new Date();
        lost.querySelector("time").dateTime = lostSince.toISOString();
        lost.hidden = false;
        tick();
      }
      // It reconnects itself unless `serve` answered with an error.
      if (source.readyState === EventSource.CLOSED) {
        setTimeout(connect, 5000);
      }
    };
  }

  tick();
  setInterval(tick, 1000);
  reveal();
  window.addEventListener("hashchange", reveal);
  if (stuck) toBottom();
  if (document.body.dataset.events) connect();
})();
