/**
 * The web UI's script (docs/specs/design_system.md): it says since when the
 * page lost `serve`, shows times and durations in the page's language and the
 * reader's time zone, keeping them up, keeps a run's transcript at its latest
 * entry while the reader is there, opens and closes fold rows by their
 * buttons, keeps each fold, fold row and confirmation as the reader opened or
 * closed it through updates, closes a confirmation as it goes ahead and a
 * "New" dialog once its thing is made, starts a "New" dialog's form afresh as
 * it closes but for the regions of the stream it holds,
 * keeps the options of the page's filters up and
 * applies the filters as they are chosen and a table's page links
 * (docs/specs/design_system.md, Tables, Pages) as a filter, but for a link the
 * reader opens elsewhere, fills in a capture's name for the
 * run chosen and keeps that run chosen through updates, adds and removes
 * New benchmark run's configurations, opens the boxes of the files of the brief
 * they give and counts their trials, collapses and expands the navigation,
 * and shows the page's tooltip.
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
   * once. A dialog whose form answers within it, such as the capture's
   * (docs/specs/runs.md, Run, Capturing), stays open to show the answer.
   */
  document.addEventListener("submit", (event) => {
    const form = event.target;
    if (form.method === "dialog" || isNewForm(form)) return;
    const dialog = form.closest?.("dialog");
    if (answersWithin(form, dialog)) return;
    dialog?.close();
  });

  /** Whether `form` puts its answer in `dialog`. */
  const answersWithin = (form, dialog) => {
    const target = form.getAttribute?.("hx-target");
    return Boolean(target && dialog?.querySelector?.(target));
  };

  /**
   * Keeps the run a capture sent chosen, as the reader chose it, through the
   * updates after a refusal (docs/specs/design_system.md, Making).
   */
  document.addEventListener("submit", (event) => {
    for (const select of event.target?.querySelectorAll?.("select[name=run]") ?? []) {
      chosenRun = select.options[select.selectedIndex]?.value ?? chosenRun;
    }
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

  /**
   * Starts the form of `dialog`, if it is a "New" dialog's, as it starts,
   * but for the regions of the page's stream it holds, which stay as the
   * stream last sent them (docs/specs/benchmarks.md, New task): its run is
   * chosen as it starts, the first, with the name suggested for it.
   */
  function startAfresh(dialog) {
    const form = dialog?.querySelector?.("form[data-new]");
    if (!form || !blanks.has(form)) return;
    const regions = Array.from(form.querySelectorAll("[data-region]"));
    form.innerHTML = blanks.get(form);
    for (const region of regions) form.querySelector(`[id="${region.id}"]`)?.replaceWith(region);
    for (const select of form.querySelectorAll("select[name=run]")) {
      chosenRun = null;
      select.selectedIndex = 0;
      suggestName(select, true);
    }
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
    if (!opening) return;
    const dialog = document.getElementById(opening.getAttribute("commandfor"));
    startAfresh(dialog);
    const launch = dialog?.querySelector?.("form.launch");
    if (!launch) return;
    openedTexts.set(launch, briefTexts());
    countTrials(launch);
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

  /**
   * Opens or closes a fold row on a click on its button, the one part of the
   * row that opens it (docs/specs/design_system.md, Tables, Opening in place).
   */
  document.addEventListener("click", (event) => {
    const clicked = event.target?.closest?.(foldButtonSelector);
    const row = clicked?.closest("tr[id]");
    const button = foldButton(row);
    if (!button || button !== clicked) return;
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
    for (const form of document.querySelectorAll("form.launch")) countTrials(form);
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
   * The page's address takes `address`, and its stream restarts at
   * `streamAt`. The stream's element is swapped out for one at `streamAt`,
   * and htmx's cleanup of the old one ends its stream (`htmx_before_cleanup`
   * in hx-sse.js).
   */
  function applyQuery(address, streamAt) {
    history.replaceState(null, "", address);
    const old = stream();
    const fresh = old.cloneNode(false);
    fresh.setAttribute("hx-sse:connect", streamAt);
    streaming = false;
    htmx.swap({ target: old, swap: "outerHTML", text: fresh.outerHTML });
  }

  /**
   * Applies `form`'s filters (docs/specs/design_system.md, Filters): the
   * page's address and its stream, at the form's `data-filters`, take their
   * query, and so drop the table's pages.
   */
  function applyFilters(form) {
    const query = filterQuery(form.elements);
    const withQuery = (path) => (query ? `${path}?${query}` : path);
    applyQuery(withQuery(location.pathname), withQuery(form.getAttribute("data-filters")));
  }

  document.addEventListener("click", (event) => {
    const link = event.target?.closest?.("nav.pages a[data-stream]");
    if (!link || event.button !== 0) return;
    if (event.ctrlKey || event.metaKey || event.shiftKey || event.altKey) return;
    event.preventDefault();
    applyQuery(link.getAttribute("href"), link.getAttribute("data-stream"));
  });

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
  // A search's own clear button and Escape empty it with no Enter, firing
  // `search`, as Enter on it also does:
  // https://developer.mozilla.org/en-US/docs/Web/API/HTMLInputElement/search_event
  applyOn(
    "search",
    (event) =>
      event.target.value === "" && addressFilters(event.target.name),
  );

  /** Whether the page's address carries a filter `name`. */
  function addressFilters(name) {
    const key = encodeURIComponent(name);
    return location.search
      .slice(1)
      .split("&")
      .some((pair) => pair.split("=")[0] === key);
  }

  /** The run the reader chose to capture, if they chose one. */
  let chosenRun = null;

  /**
   * Fills the name of the capture of run choice `select` with the name
   * suggested for the run chosen (docs/specs/benchmarks.md, New task), unless,
   * but for `always`, the name holds one the reader typed: one suggested for
   * none of the runs.
   */
  function suggestName(select, always) {
    const name = Array.from(select.form?.elements ?? []).find((field) => field.name === "name");
    const suggested = (option) => option.getAttribute("data-name");
    const chosen = select.options[select.selectedIndex];
    if (!name || !chosen) return;
    if (always || name.value === "" || Array.from(select.options, suggested).includes(name.value)) {
      name.value = suggested(chosen) ?? name.value;
    }
  }

  document.addEventListener("change", (event) => {
    const select = event.target;
    if (select?.tagName !== "SELECT" || select.name !== "run" || !select.form) return;
    const chosen = select.options[select.selectedIndex];
    if (!chosen) return;
    chosenRun = chosen.value;
    suggestName(select, false);
  });

  /**
   * Chooses again the run the reader chose to capture, while an update
   * still offers it, and suggests a name for the run chosen.
   */
  function keepRun() {
    for (const select of document.querySelectorAll("select[name=run]")) {
      const at = Array.from(select.options).findIndex((option) => option.value === chosenRun);
      if (at !== -1) select.selectedIndex = at;
      suggestName(select, false);
    }
  }

  /**
   * The texts of the files of the brief, by path, as a run of the stage uses
   * them, as the stream last sent them.
   */
  const briefTexts = () =>
    JSON.parse(document.getElementById("brief-texts")?.getAttribute("data-texts") || "{}");

  /**
   * The texts of the files of the brief as New benchmark run's form `form` was
   * opened, by form, from which each file it opens starts
   * (docs/specs/benchmarks.md, New benchmark run, Files of the brief).
   */
  const openedTexts = new Map();

  /** The number the last configuration added took, for its fields' names. */
  let configurations = 0;

  /** `n` and the noun for one or for many. */
  const counted = (n, one, many) => `${n} ${n === 1 ? one : many}`;

  /**
   * Says how many trials New benchmark run's form `form` makes, of the tasks
   * the benchmark has as the stream last sent them.
   */
  function countTrials(form) {
    const tasks =
      Number(document.getElementById("new-benchmark-run-offer")?.getAttribute("data-tasks")) || 0;
    const configured = form.querySelectorAll("fieldset.configuration").length;
    const typed = form.elements.repetitions.value.trim();
    const repetitions = /^[0-9]+$/.test(typed) ? Math.max(1, Number(typed)) : 1;
    form.querySelector(".trial-count").textContent =
      `${counted(configured, "configuration", "configurations")} × ` +
      `${counted(tasks, "task", "tasks")} × ` +
      `${counted(repetitions, "repetition", "repetitions")}: ` +
      `${counted(configured * tasks * repetitions, "trial", "trials")}`;
  }

  document.addEventListener("input", (event) => {
    const form = event.target?.closest?.("form.launch");
    if (form && event.target.name === "repetitions") countTrials(form);
  });

  /**
   * Fills in what the server drew with `__N__` for a number and `__PATH__`
   * for a path, in `element` and the fields and labels it holds, with `n`
   * and `path`.
   */
  function fillIn(element, n, path) {
    const filled = (text) => text.replaceAll("__N__", n).replaceAll("__PATH__", path);
    for (const field of [element, ...element.querySelectorAll("[name], [id], label")]) {
      if (field.name) field.name = filled(field.name);
      if (field.id) field.id = filled(field.id);
      if (field.htmlFor) field.htmlFor = filled(field.htmlFor);
      if (field.tagName === "LABEL" && !field.children.length) {
        field.textContent = filled(field.textContent);
      }
    }
  }

  /** The texts of the files of the brief New benchmark run's form `form` starts from. */
  const startingTexts = (form) => openedTexts.get(form) ?? briefTexts();

  /**
   * Adds a configuration at the end of `form` from the page's template,
   * numbered past every configuration it holds, offering the files of the
   * brief the form starts from.
   */
  function addConfiguration(form) {
    const template = document.getElementById("configuration-template");
    const held = Array.from(form.querySelectorAll("fieldset.configuration[data-n]"), (fieldset) =>
      Number(fieldset.dataset.n),
    );
    configurations = Math.max(configurations, ...held) + 1;
    const n = String(configurations);
    const fieldset = template.content.firstElementChild.cloneNode(true);
    fieldset.dataset.n = n;
    fillIn(fieldset, n, "");
    const choice = fieldset.querySelector("select.brief-file");
    const unchosen = choice.options[0];
    choice.replaceChildren(
      unchosen,
      ...Object.keys(startingTexts(form)).map((path) => new Option(path, path)),
    );
    form.querySelector(".configurations").append(fieldset);
    fieldset.querySelector("input")?.focus();
    countTrials(form);
  }

  document.addEventListener("click", (event) => {
    const button = event.target?.closest?.("button");
    const form = button?.closest("form.launch");
    if (!form) return;
    if (button.classList.contains("add-configuration")) {
      addConfiguration(form);
    } else if (button.classList.contains("remove-configuration")) {
      button.closest("fieldset.configuration").remove();
      countTrials(form);
    }
  });

  /**
   * The box each choice of a file of the brief last opened, while the reader
   * has not changed its text, by choice: choosing another file closes it, so
   * that the arrow keys, which choose each file they pass, leave a box only
   * for the file they stop on.
   */
  const untouched = new Map();

  /** Whether the text of `box` is still the text it started with. */
  function isUntouched(box) {
    const area = box.querySelector("textarea");
    const started = box.querySelector("input[type=hidden]");
    return !!area && !!started && area.value === started.value;
  }

  /**
   * Opens the box of the file of the brief chosen in `select`, unless its
   * configuration has one, its text starting as the dialog was opened
   * (docs/specs/benchmarks.md, New benchmark run, Files of the brief).
   */
  function openFile(select) {
    const path = select.value;
    const last = untouched.get(select);
    untouched.delete(select);
    if (last && isUntouched(last)) last.remove();
    if (!path) return;
    const form = select.closest("form.launch");
    const fieldset = select.closest("fieldset.configuration");
    const n = fieldset.dataset.n;
    const files = fieldset.querySelector(".files");
    const name = `c${n}.briefs/${path}`;
    if (Array.from(files.querySelectorAll("textarea")).some((area) => area.name === name)) return;
    const box = document
      .getElementById("brief-file-template")
      .content.firstElementChild.cloneNode(true);
    fillIn(box, n, path);
    const text = startingTexts(form)[path] ?? "";
    box.querySelector("textarea").value = text;
    box.querySelector("input[type=hidden]").value = text;
    files.append(box);
    untouched.set(select, box);
  }

  document.addEventListener("change", (event) => {
    if (event.target?.classList?.contains("brief-file")) openFile(event.target);
  });

  /**
   * Collapses the sidebar to the rail, or expands the rail to the sidebar,
   * on a click on the navigation's foot button, which then names what it does
   * next, and posts the choice to the URL its `data-navigation-toggle` holds,
   * without waiting, for the server to remember it in a cookie for a year
   * (docs/specs/web/shell.md, Expanding and collapsing, Remembered). A cookie
   * written here would last only 7 days in Safari
   * (https://webkit.org/tracking-prevention/). Each choice is posted once the
   * one before it is answered, so the cookie set last is the last choice.
   */
  let remembering = Promise.resolve();
  document.addEventListener("click", (event) => {
    const toggle = event.target?.closest?.("button[data-navigation-toggle]");
    if (!toggle) return;
    const drawer = toggle.closest(".drawer");
    const rail = drawer.getAttribute("data-navigation") !== "rail";
    if (rail) drawer.setAttribute("data-navigation", "rail");
    else drawer.removeAttribute("data-navigation");
    document.getElementById(toggle.getAttribute("aria-labelledby")).textContent = rail
      ? "Expand navigation"
      : "Collapse navigation";
    const url = toggle.getAttribute("data-navigation-toggle");
    const navigation = rail ? "rail" : "sidebar";
    remembering = remembering
      .then(() =>
        fetch(url, { method: "POST", body: new URLSearchParams({ navigation }), keepalive: true }),
      )
      .catch(() => {});
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
