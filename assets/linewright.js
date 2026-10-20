// Linewright's own script for the web UI (docs/specs/design_system.md): it
// keeps the page up from its event stream, says since when it lost `serve`,
// and shows times in the reader's time zone, keeping them up.
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

  function connect() {
    const source = new EventSource(document.body.dataset.events);
    source.onopen = () => {
      lostSince = null;
      lost.hidden = true;
    };
    source.onmessage = (event) => {
      htmx.swap({ text: event.data, sourceElement: document.body }).then(tick);
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
  if (document.body.dataset.events) connect();
})();
