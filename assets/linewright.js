/**
 * The web UI's script (docs/specs/design_system.md): it says since when the
 * page lost `serve`, shows times and durations in the page's language and the
 * reader's time zone, keeping them up, keeps a run's transcript at its latest
 * entry while the reader is there, opens and closes fold rows, keeps each
 * fold, fold row and confirmation as the reader opened or closed it through
 * updates, closes a confirmation as it goes ahead and a "New" dialog once
 * its thing is made, starts a "New" dialog's form afresh as it closes,
 * keeps the options of the page's filters up and
 * applies the filters as they are chosen, fills in a capture's name for the
 * run chosen and keeps that run chosen through updates, builds a launch's
 * configurations and counts its replays, and shows the page's tooltip.
 */
(() => {
  "use strict";

  // A hook that returns false keeps the attribute from being morphed:
  // https://four.htmx.org/docs/extension-authoring-guide#event-hooks
  htmx.registerExtension("linewright-folds", {
    htmx_before_morph_attr: (element, detail) => !leftByReader(element, detail.attrName),
  });

  /**
   * Whether `attrName` of `element` is as the reader left it, opened or
   * closed: a fold's or a confirmation's `open`, a fold row's button's
   * `aria-expanded` or the `hidden` of a row under a fold row.
   */
  function leftByReader(element, attrName) {
    switch (element.tagName) {
      case "DETAILS":
      case "DIALOG":
        return attrName === "open";
      case "BUTTON":
        return attrName === "aria-expanded" && element.hasAttribute("aria-controls");
      case "TR":
        return attrName === "hidden" && element.hasAttribute("data-fold-of");
      default:
        return false;
    }
  }

  const lang = document.documentElement.lang;
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

  /**
   * A past time `at` as of `now` in words, rounded down: `just now`, then
   * minutes, hours or days up to 30 days, then its date, with its year where
   * that is not this year's.
   */
  function ago(at, now) {
    const seconds = Math.floor((now - at) / 1000);
    if (seconds < 60) return "just now";
    if (seconds < 3600) return relative.format(-Math.floor(seconds / 60), "minute");
    if (seconds < 86400) return relative.format(-Math.floor(seconds / 3600), "hour");
    if (seconds <= 30 * 86400) return relative.format(-Math.floor(seconds / 86400), "day");
    const thisYear = new Date(at).getFullYear() === new Date(now).getFullYear();
    return `on ${(thisYear ? dateThisYear : dateWithYear).format(at)}`;
  }

  const durations = Intl.DurationFormat && new Intl.DurationFormat(lang, { style: "narrow" });
  const zeroDuration = Intl.DurationFormat &&
    new Intl.DurationFormat(lang, { style: "narrow", secondsDisplay: "always" });
  /** A settled duration's `time`, whose `datetime` is `PT{seconds}S`. */
  const settledDuration = '[datetime^="PT"]';

  /**
   * `seconds` in narrow units, balanced into days, hours, minutes and
   * seconds, of which it shows the two largest that are not zero (`1d 5m`).
   */
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

  /**
   * Shows a past time relative, with the absolute time in the page's tooltip;
   * a future one absolute, with no tip; and, where the browser has
   * Intl.DurationFormat, a settled duration from its seconds and a running one
   * as long as it has run.
   */
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

  const latest = document.getElementById("latest");
  const atBottom = () =>
    window.innerHeight + window.scrollY >= document.documentElement.scrollHeight - 40;
  /** The element whose `hx-sse:connect` is the page's event stream. */
  const stream = () => document.getElementById("stream");
  const streamPath = () => stream()?.getAttribute("hx-sse:connect") || "";
  let stuck = streamPath().includes("follow=");

  function toBottom() {
    window.scrollTo(0, document.documentElement.scrollHeight);
  }

  /** Whether the reader left each fold or fold row open, by its id. */
  const folds = new Map();

  // `toggle` does not bubble: https://html.spec.whatwg.org/multipage/interactive-elements.html#the-details-element
  document.addEventListener(
    "toggle",
    (event) => {
      if (event.target.tagName === "DETAILS" && event.target.id) {
        folds.set(event.target.id, event.target.open);
      }
    },
    true,
  );

  /**
   * Closes a confirmation as the reader goes ahead
   * (docs/specs/design_system.md, Confirming), so that it sends its request
   * once.
   */
  document.addEventListener("submit", (event) => {
    const form = event.target;
    if (form.method === "dialog" || isNewForm(form)) return;
    form.closest?.("dialog")?.close();
  });

  /**
   * Whether `form` is a "New" dialog's (docs/specs/design_system.md, Making),
   * which answers into itself and stays open on a refusal.
   */
  const isNewForm = (form) => form.hasAttribute?.("data-new") ?? false;

  /** What each "New" dialog's form holds as it starts, by form. */
  const blanks = new Map(
    Array.from(document.querySelectorAll("dialog form[data-new]"), (form) => [form, form.innerHTML]),
  );

  /** Starts the form of `dialog`, if it is a "New" dialog's, as it starts. */
  function startAfresh(dialog) {
    const form = dialog?.querySelector?.("form[data-new]");
    if (form && blanks.has(form)) form.innerHTML = blanks.get(form);
  }

  /** Closes a "New" dialog once its thing is made. */
  document.addEventListener("made", (event) => event.target?.closest?.("dialog")?.close());

  /**
   * Starts a "New" dialog's form afresh as the dialog closes, however it
   * closes, and as it opens, after any answer that came once it had closed.
   * `close` does not bubble:
   * https://html.spec.whatwg.org/multipage/interactive-elements.html#the-dialog-element
   */
  document.addEventListener("close", (event) => startAfresh(event.target), true);
  document.addEventListener("click", (event) => {
    const opening = event.target?.closest?.("button[command=show-modal]");
    if (opening) startAfresh(document.getElementById(opening.getAttribute("commandfor")));
  });

  /** A fold row's button. */
  const foldButtonSelector = "button[aria-expanded][aria-controls]";

  /** The button in the first cell of fold row `row` that opens it, if any. */
  const foldButton = (row) => row?.querySelector?.(`:scope > :first-child > ${foldButtonSelector}`);

  /** Whether fold row `id` shows the rows under it: it is open and shows itself. */
  function showsUnder(id) {
    const row = document.getElementById(id);
    if (foldButton(row)?.getAttribute("aria-expanded") !== "true") return false;
    const under = row.getAttribute("data-fold-of");
    return !under || showsUnder(under);
  }

  /** Shows each row under a fold row while every row it is under shows its rows. */
  function showFolded() {
    for (const row of document.querySelectorAll("tr[data-fold-of]")) {
      const hidden = !showsUnder(row.getAttribute("data-fold-of"));
      if (row.hidden !== hidden) row.hidden = hidden;
    }
  }

  /** What a click on acts on by itself, rather than on the row it is in. */
  const interactive = "a, button, input, select, textarea, label, summary, dialog";

  /**
   * Opens or closes a fold row on a click on it, but for a click on what acts
   * by itself, such as a link (docs/specs/benchmarks.md, Suite, Cases).
   */
  document.addEventListener("click", (event) => {
    const row = event.target?.closest?.("tr[id]");
    const button = foldButton(row);
    if (!button || (event.target.closest(interactive) ?? button) !== button) return;
    const open = button.getAttribute("aria-expanded") !== "true";
    button.setAttribute("aria-expanded", String(open));
    folds.set(row.id, open);
    showFolded();
  });

  /** Opens or closes each fold and fold row the reader toggled, as they left it. */
  function keepFolds() {
    for (const [id, open] of folds) {
      const fold = document.getElementById(id);
      if (fold?.tagName === "DETAILS") {
        if (fold.open !== open) fold.open = open;
      } else if (fold?.tagName === "TR") {
        foldButton(fold)?.setAttribute("aria-expanded", String(open));
      }
    }
    showFolded();
  }

  /**
   * Fills each select with `data-options` from the element whose id it names
   * (docs/specs/board.md, Filtering by person, The people): its first, unset
   * option, then the element's options in their order, with whichever is chosen
   * still chosen, its own option kept in its place where the element lacks it.
   * Setting a select's options or value fires no `change`:
   * https://html.spec.whatwg.org/multipage/form-elements.html#send-select-update-notifications
   */
  function fillOptions() {
    for (const select of document.querySelectorAll("select[data-options]")) {
      const source = document.getElementById(select.getAttribute("data-options"));
      if (!source) continue;
      const chosen = select.value;
      const [unset, ...old] = Array.from(select.options);
      const options = Array.from(
        source.querySelectorAll("option"),
        ({ value }) => old.find((option) => option.value === value) || new Option(value, value),
      );
      const at = old.findIndex((option) => option.value === chosen);
      if (at !== -1 && !options.some((option) => option.value === chosen)) {
        options.splice(at, 0, old[at]);
      }
      select.replaceChildren(unset, ...options);
      select.value = chosen;
    }
  }

  /**
   * Brings the page up to date after a message, `entries` whether it added
   * to the transcript.
   */
  function swapped(entries) {
    fillOptions();
    keepFolds();
    keepRun();
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

  /**
   * The event ID of the last message; a message with a different ID added
   * entries to the transcript.
   */
  let lastEntryId = "";
  /** Whether the `hx-sse` extension has taken over the page's event stream. */
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
  // hx-sse reconnects only a stream that connected once (`sse.reconnect`):
  // https://four.htmx.org/extensions/hx-sse
  document.body.addEventListener("htmx:finally:request", (event) => {
    if (event.target !== stream() || streaming) return;
    lose();
    setTimeout(() => {
      const from = stream();
      if (from === event.target) htmx.ajax("GET", streamPath(), { source: from });
    }, 5000);
  });

  /**
   * The query of the filters among `fields` that are set, each trimmed, or
   * `""` when none is.
   */
  function filterQuery(fields) {
    return Array.from(fields)
      .filter((field) => field.name)
      .map((field) => [field.name, field.value.trim()])
      .filter(([, value]) => value !== "")
      .map(([name, value]) => `${encodeURIComponent(name)}=${encodeURIComponent(value)}`)
      .join("&");
  }

  /**
   * Applies `form`'s filters (docs/specs/design_system.md, Filters): the
   * page's address and its stream, at the form's `data-filters`, take their
   * query. The stream's element is swapped out for one at the new query, and
   * htmx's cleanup of the old one ends its stream (`htmx_before_cleanup` in
   * hx-sse.js).
   */
  function applyFilters(form) {
    const query = filterQuery(form.elements);
    const withQuery = (path) => (query ? `${path}?${query}` : path);
    history.replaceState(null, "", withQuery(location.pathname));
    const old = stream();
    const fresh = old.cloneNode(false);
    fresh.setAttribute("hx-sse:connect", withQuery(form.getAttribute("data-filters")));
    streaming = false;
    htmx.swap({ target: old, swap: "outerHTML", text: fresh.outerHTML });
  }

  /**
   * Applies the filters of the form an event of `type` came from, for an
   * event `chosen` says chose a filter.
   */
  function applyOn(type, chosen) {
    document.addEventListener(type, (event) => {
      const form = event.target?.closest?.("form[data-filters]");
      if (!form || !chosen(event)) return;
      event.preventDefault();
      applyFilters(form);
    });
  }

  applyOn("change", (event) => event.target.tagName === "SELECT");
  // A form with more than one field to type in submits on no Enter:
  // https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#implicit-submission
  applyOn("keydown", (event) => event.key === "Enter" && event.target.tagName === "INPUT");

  /** The run the reader chose to capture, if they chose one. */
  let chosenRun = null;

  /**
   * Fills a capture's name with the name suggested for the run chosen
   * (docs/specs/benchmarks.md, Suite), unless the name holds one
   * the reader typed: one suggested for none of the runs.
   */
  document.addEventListener("change", (event) => {
    const select = event.target;
    if (select?.tagName !== "SELECT" || select.name !== "run" || !select.form) return;
    const name = Array.from(select.form.elements).find((field) => field.name === "name");
    const suggested = (option) => option.getAttribute("data-name");
    const chosen = select.options[select.selectedIndex];
    if (!chosen) return;
    chosenRun = chosen.value;
    if (!name) return;
    if (name.value === "" || Array.from(select.options, suggested).includes(name.value)) {
      name.value = suggested(chosen) ?? name.value;
    }
  });

  /**
   * Chooses again the run the reader chose to capture, while an update
   * still offers it.
   */
  function keepRun() {
    for (const select of document.querySelectorAll("select[name=run]")) {
      const at = Array.from(select.options).findIndex((option) => option.value === chosenRun);
      if (at !== -1) select.selectedIndex = at;
    }
  }

  /**
   * The launch form's configurations (docs/specs/benchmarks.md, Suite,
   * Launching): each added from the page's template, numbered for its fields'
   * names, and each file of the brief it changes starting as the replays would
   * have it without the configuration.
   */
  let configurations = 0;

  /** The texts of the files of the brief, by path, as the form starts them. */
  const briefTexts = () => JSON.parse(document.getElementById("brief-texts")?.textContent || "{}");

  /** `n` and the noun for one or for many. */
  const counted = (n, one, many) => `${n} ${n === 1 ? one : many}`;

  /** Says how many replays the launch `form` makes. */
  function countReplays(form) {
    const cases = Number(form.getAttribute("data-cases")) || 0;
    const configured = form.querySelectorAll("fieldset.configuration").length;
    const typed = form.elements.repetitions.value.trim();
    const repetitions = /^[0-9]+$/.test(typed) ? Math.max(1, Number(typed)) : 1;
    form.querySelector(".replay-count").textContent =
      `${counted(cases * configured * repetitions, "replay", "replays")}: ` +
      `${counted(cases, "case", "cases")}, ` +
      `${counted(configured, "configuration", "configurations")}, ` +
      `${counted(repetitions, "repetition", "repetitions")}`;
  }

  document.addEventListener("input", (event) => {
    const form = event.target?.closest?.("form.launch");
    if (form && event.target.name === "repetitions") countReplays(form);
  });

  document.addEventListener("click", (event) => {
    const button = event.target?.closest?.("button");
    const form = button?.closest("form.launch");
    if (!form) return;
    if (button.classList.contains("add-configuration")) {
      const template = document.getElementById("configuration-template");
      configurations += 1;
      const n = String(configurations);
      const fieldset = template.content.firstElementChild.cloneNode(true);
      fieldset.dataset.n = n;
      for (const field of fieldset.querySelectorAll("[name]")) {
        field.name = field.name.replace("__N__", n);
      }
      form.querySelector(".configurations").append(fieldset);
      fieldset.querySelector("input")?.focus();
      countReplays(form);
    } else if (button.classList.contains("remove-configuration")) {
      button.closest("fieldset.configuration").remove();
      countReplays(form);
    } else if (button.classList.contains("add-file")) {
      const fieldset = button.closest("fieldset.configuration");
      const path = fieldset.querySelector("select.brief-file").value;
      if (!path) return;
      const name = `c${fieldset.dataset.n}.briefs/${path}`;
      const files = fieldset.querySelector(".files");
      const given = Array.from(files.querySelectorAll("textarea")).find((area) => area.name === name);
      if (given) {
        given.focus();
        return;
      }
      const label = document.createElement("label");
      label.className = "flex flex-col gap-1";
      const caption = document.createElement("span");
      caption.className = "text-muted text-sm";
      caption.textContent = path;
      const area = document.createElement("textarea");
      area.className = "textarea textarea-sm w-full font-mono";
      area.name = name;
      area.rows = 12;
      const text = briefTexts()[path] ?? "";
      area.value = text;
      const started = document.createElement("input");
      started.type = "hidden";
      started.name = `c${fieldset.dataset.n}.started/${path}`;
      started.value = text;
      label.append(caption, area, started);
      files.append(label);
      area.focus();
    }
  });

  /**
   * The page's one tooltip, which shows the tip of one trigger at a time: an
   * element with `data-tip`, which always shows it, or `data-tip-cut`, which
   * shows its `data-tip`, or else its text, only while it or an element in it
   * is cut. It shows on hover and on keyboard focus, over the page, until the
   * pointer leaves it or focus moves, and Escape hides it
   * (`docs/specs/design_system.md`, Layout; WCAG 1.4.13,
   * https://www.w3.org/WAI/WCAG22/Understanding/content-on-hover-or-focus).
   */
  const tip = document.getElementById("tip");
  /** The trigger whose tip shows, if any. */
  let shownFor = null;
  /** The trigger whose tip Escape hid, until the pointer leaves it or focus moves. */
  let dismissed = null;
  /** The pending hide after the pointer left the trigger and the tip. */
  let leaving = null;
  /** How long, in milliseconds, the pointer may be off the trigger and the tip before the tip hides. */
  const leaveDelay = 100;
  /** How near, in pixels, the viewport's edges the tip may come. */
  const edge = 8;

  const isCut = (element) =>
    [element, ...element.querySelectorAll("*")].some(
      (cut) => cut.scrollWidth > cut.clientWidth || cut.scrollHeight > cut.clientHeight,
    );

  /**
   * The trigger that shows for `target`: of the triggers it is in, the
   * outermost cut text, which shows all its triggers hold, or else the
   * innermost that always shows.
   */
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

  /** The cut text that `focused` holds, if any, which shows for it on keyboard focus. */
  const cutIn = (focused) =>
    [...focused.querySelectorAll("*")].find(
      (element) => element.hasAttribute("data-tip-cut") && isCut(element),
    ) || null;

  const textOf = (trigger) =>
    trigger.getAttribute("data-tip") ?? trigger.textContent.replace(/\s+/g, " ").trim();

  /**
   * Whether `trigger`'s tip is in neither its text nor its label, so that
   * the tip describes it while it shows:
   * https://www.w3.org/WAI/ARIA/apg/patterns/tooltip/
   */
  const describes = (trigger) =>
    !trigger.hasAttribute("data-tip-cut") && !trigger.hasAttribute("aria-label");

  /** Makes `trigger`, or none, the trigger whose tip shows. */
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

  /**
   * Where the tip goes for a trigger whose box is `box`, drawn by daisyUI
   * above it as `drawn`, its width, height and distance from the box's
   * bottom, in a viewport `viewportWidth` wide: below the box where there is
   * no room above, and `shift` along it to stay inside the viewport.
   */
  function placement(box, drawn, viewportWidth) {
    const below = box.bottom - drawn.bottom - drawn.height < 0;
    const left = box.left + box.width / 2 - drawn.width / 2;
    const shift =
      Math.max(0, edge - left) - Math.max(0, left + drawn.width - (viewportWidth - edge));
    return { below, shift };
  }

  /** Places the tip at its trigger as `placement` says. */
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

  /**
   * Hides the tip if its trigger is gone or no longer shows, and otherwise
   * shows what the trigger now holds where it now is.
   */
  function refreshTip() {
    if (!shownFor) return;
    const trigger = shownFor.isConnected && triggerOf(shownFor);
    if (trigger) show(trigger);
    else hide();
  }

  /** Whether `node` is on the tip or on the trigger it shows for. */
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
  if (stuck) toBottom();
})();
