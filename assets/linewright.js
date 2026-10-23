// Linewright's own script for the web UI (docs/specs/design_system.md): it
// says since when the page lost `serve`, shows times and durations in the
// page's language and the reader's time zone, keeping them up, keeps a run's
// transcript at its latest entry while the reader is there, keeps each
// fold as the reader opened or closed it through updates, and shows the
// page's tooltip. htmx's
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

  // A past time relative, with the absolute time in the page's tooltip; a
  // future one absolute, with no tip; a settled duration from its seconds,
  // and a running one as long as it has run.
  function tick() {
    const now = Date.now();
    for (const time of document.querySelectorAll(`time[datetime]:not(${settledDuration})`)) {
      const at = Date.parse(time.dateTime);
      if (Number.isNaN(at)) continue;
      const shown = absolute.format(at);
      if (at <= now) {
        time.textContent = ago(at, now);
        time.dataset.tip = shown;
      } else {
        time.textContent = shown;
        time.removeAttribute("data-tip");
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
    refreshTip();
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

  // The page's one tooltip, `#tip`, shows the tip of one trigger at a time:
  // an element with `data-tip`, which always shows it, or `data-tip-cut`,
  // which shows its `data-tip`, or else its text, only while it or an element
  // in it is cut. It shows on hover and on keyboard focus, over the page,
  // until the pointer leaves it or focus moves, and Escape hides it
  // (`docs/specs/design_system.md`, Layout; WCAG 1.4.13,
  // https://www.w3.org/WAI/WCAG22/Understanding/content-on-hover-or-focus).
  // Its listeners are the document's, so a trigger that an update brings
  // works as one the page loaded with.
  const tip = document.getElementById("tip");
  // The trigger whose tip shows, if any.
  let shownFor = null;
  // The trigger whose tip Escape hid, until the pointer leaves it or focus
  // moves.
  let dismissed = null;
  // The pending hide after the pointer left the trigger and the tip.
  let leaving = null;
  // How long the pointer may be off the trigger and the tip before the tip
  // hides, so that it can cross the gap between them.
  const leaveDelay = 100;
  // How near the viewport's edges the tip may come.
  const edge = 8;

  const isCut = (element) =>
    [element, ...element.querySelectorAll("*")].some(
      (cut) => cut.scrollWidth > cut.clientWidth || cut.scrollHeight > cut.clientHeight,
    );

  // The trigger that shows for `target`: of the triggers it is in, the
  // outermost cut text, which shows all its triggers hold, or else the
  // innermost that always shows.
  function triggerOf(target) {
    let innermost = null;
    let outermostCut = null;
    for (let element = target; element && element.hasAttribute; element = element.parentElement) {
      if (element.hasAttribute("data-tip-cut")) {
        if (isCut(element)) outermostCut = element;
      } else if (!innermost && element.hasAttribute("data-tip")) {
        innermost = element;
      }
    }
    return outermostCut || innermost;
  }

  // The cut text that `focused` holds, if any, which shows for it on
  // keyboard focus: a finished lane's `summary` takes focus itself, and its
  // head's text is in it.
  const cutIn = (focused) =>
    [...focused.querySelectorAll("*")].find(
      (element) => element.hasAttribute("data-tip-cut") && isCut(element),
    ) || null;

  const textOf = (trigger) =>
    trigger.getAttribute("data-tip") ?? trigger.textContent.replace(/\s+/g, " ").trim();

  // While its tip shows, a trigger whose tip is in neither its text nor its
  // label, such as a past time, is described by it, so that assistive tech
  // reads it too: https://www.w3.org/WAI/ARIA/apg/patterns/tooltip/
  // Cut text holds its whole text, and a labelled trigger its tip.
  const describes = (trigger) =>
    !trigger.hasAttribute("data-tip-cut") && !trigger.hasAttribute("aria-label");

  // `shownFor` becomes `trigger`, or none.
  function showFor(trigger) {
    if (shownFor && shownFor !== trigger) shownFor.removeAttribute("aria-describedby");
    shownFor = trigger;
    if (trigger && describes(trigger)) trigger.setAttribute("aria-describedby", tip.id);
  }

  function cancelLeaving() {
    clearTimeout(leaving);
    leaving = null;
  }

  function hide() {
    cancelLeaving();
    if (!shownFor) return;
    showFor(null);
    tip.hidePopover();
    tip.dataset.tip = "";
  }

  function show(trigger) {
    cancelLeaving();
    if (trigger === dismissed) return;
    const text = textOf(trigger);
    if (!text) return hide();
    tip.dataset.tip = text;
    if (!shownFor) tip.showPopover();
    showFor(trigger);
    place();
  }

  // Where the tip goes for a trigger whose box is `box`, drawn by daisyUI
  // above it as `drawn`, its width, height and distance from the box's
  // bottom, in a viewport `viewportWidth` wide: below the box where there is
  // no room above, and `shift` along it to stay inside the viewport.
  function placement(box, drawn, viewportWidth) {
    const below = box.bottom - drawn.bottom - drawn.height < 0;
    const left = box.left + box.width / 2 - drawn.width / 2;
    const shift =
      Math.max(0, edge - left) - Math.max(0, left + drawn.width - (viewportWidth - edge));
    return { below, shift };
  }

  // Lays the tip's box over its trigger's, so that daisyUI draws the tip
  // over the trigger as `placement` says.
  function place() {
    if (!shownFor) return;
    const box = shownFor.getBoundingClientRect();
    Object.assign(tip.style, {
      left: `${box.left}px`,
      top: `${box.top}px`,
      width: `${box.width}px`,
      height: `${box.height}px`,
    });
    tip.style.removeProperty("--tt-trans");
    tip.classList.toggle("tooltip-bottom", false);
    tip.classList.toggle("tooltip-top", true);
    const drawn = getComputedStyle(tip, "::before");
    const { below, shift } = placement(
      box,
      {
        width: parseFloat(drawn.width),
        height: parseFloat(drawn.height),
        bottom: parseFloat(drawn.bottom),
      },
      window.innerWidth,
    );
    tip.classList.toggle("tooltip-top", !below);
    tip.classList.toggle("tooltip-bottom", below);
    if (shift) tip.style.setProperty("--tt-trans", `calc(-50% + ${shift}px)`);
  }

  // After an update, the tip hides if its trigger is gone or no longer
  // shows, and otherwise shows what the trigger now holds where it now is.
  function refreshTip() {
    if (!shownFor) return;
    const trigger = shownFor.isConnected && triggerOf(shownFor);
    if (trigger) show(trigger);
    else hide();
  }

  // Whether `node` is on the tip or on the trigger it shows for.
  const onShown = (node) => node === tip || (shownFor && shownFor.contains(node));

  document.addEventListener("pointerover", (event) => {
    const trigger = event.target === tip ? shownFor : triggerOf(event.target);
    if (!trigger) return;
    if (trigger === shownFor) cancelLeaving();
    else show(trigger);
  });
  document.addEventListener("pointerout", (event) => {
    const leaves = (element) =>
      element.contains(event.target) && !element.contains(event.relatedTarget);
    if (dismissed && leaves(dismissed)) dismissed = null;
    if (!shownFor || !onShown(event.target) || onShown(event.relatedTarget)) return;
    cancelLeaving();
    leaving = setTimeout(hide, leaveDelay);
  });
  document.addEventListener("focusin", (event) => {
    dismissed = null;
    if (!event.target.matches(":focus-visible")) return;
    const trigger = triggerOf(event.target) || cutIn(event.target);
    if (trigger) show(trigger);
  });
  document.addEventListener("focusout", (event) => {
    dismissed = null;
    if (shownFor && (shownFor.contains(event.target) || event.target.contains(shownFor))) hide();
  });
  document.addEventListener("keydown", (event) => {
    if (event.key !== "Escape" || !shownFor) return;
    dismissed = shownFor;
    hide();
  });
  document.addEventListener("scroll", place, true);
  window.addEventListener("resize", place);

  tick();
  setInterval(tick, 1000);
  reveal();
  window.addEventListener("hashchange", reveal);
  if (stuck) toBottom();
})();
