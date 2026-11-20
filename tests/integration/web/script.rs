//! The page's script, run against stand-ins for the browser and for htmx:
//! the hooks it gives htmx, as htmx 4 calls them.

use boa_engine::{Context, Source};

use super::*;

/// Stand-ins for what the script reads of the browser at load, with
/// `htmx.registerExtension` keeping each extension by name in `extensions`.
const BROWSER: &str = r"
var extensions = {};
var htmx = { registerExtension: (name, extension) => { extensions[name] = extension; } };
var Intl = { RelativeTimeFormat: function () {}, DateTimeFormat: function () {} };
var element = { addEventListener() {}, getAttribute() { return null; } };
var document = {
  addEventListener() {},
  body: element,
  documentElement: { scrollHeight: 0 },
  getElementById() { return null; },
  querySelectorAll() { return []; },
};
var window = { addEventListener() {}, innerHeight: 0, scrollY: 0 };
function setInterval() {}
function setTimeout() {}
";

/// The script the board serves.
async fn served_script() -> String {
    let pages = Pages::of(view(vec![], vec![])).await;
    let html = pages.page("/").await;
    let src = select(&html, "script[src]")
        .into_iter()
        .filter_map(|script| script.attr("src"))
        .find(|src| src.starts_with("/assets/linewright-"))
        .expect("the page's script")
        .to_string();
    let (status, script) = pages.get(&src).await;
    assert_eq!(status, 200);
    script
}

/// What the last of `sources` evaluates to, as a string, after the rest.
fn evaluated(sources: &[&str]) -> String {
    let mut context = Context::default();
    let (last, rest) = sources.split_last().unwrap();
    for source in rest {
        context.eval(Source::from_bytes(source)).unwrap();
    }
    let answer = context.eval(Source::from_bytes(last)).unwrap();
    answer
        .to_string(&mut context)
        .unwrap()
        .to_std_string_escaped()
}

/// As [`evaluated`], with the jobs each of the rest queues, such as a
/// promise's callbacks, run after it.
fn settled(sources: &[&str]) -> String {
    let mut context = Context::default();
    let (last, rest) = sources.split_last().unwrap();
    for source in rest {
        context.eval(Source::from_bytes(source)).unwrap();
        context.run_jobs().unwrap();
    }
    let answer = context.eval(Source::from_bytes(last)).unwrap();
    answer
        .to_string(&mut context)
        .unwrap()
        .to_std_string_escaped()
}

#[test_log::test(tokio::test)]
async fn an_update_neither_opens_nor_closes_a_fold_a_fold_row_or_a_confirmation_and_morphs_every_other_attribute()
 {
    let script = served_script().await;

    // Each hook as htmx calls it in a morph:
    // https://four.htmx.org/docs/extension-authoring-guide#event-hooks
    let asked = r#"
const hooks = Object.values(extensions)
  .map((extension) => extension.htmx_before_morph_attr)
  .filter(Boolean);
const morphed = (tagName, attributes, attrName, newValue) => {
  const element = { tagName, hasAttribute: (name) => attributes.includes(name) };
  return hooks.every((hook) => hook(element, { attrName, newValue }) !== false);
};
[
  ["DETAILS", [], "open", ""],
  ["DETAILS", [], "open", null],
  ["DETAILS", [], "class", "lane"],
  ["DIALOG", [], "open", ""],
  ["DIALOG", [], "open", null],
  ["SECTION", [], "open", null],
  ["BUTTON", ["aria-controls"], "aria-expanded", "false"],
  ["BUTTON", ["aria-controls"], "class", "fold"],
  ["BUTTON", [], "aria-expanded", "false"],
  ["TR", ["data-fold-of"], "hidden", ""],
  ["TR", ["data-fold-of"], "hidden", null],
  ["TR", [], "hidden", ""],
].map(([tag, attributes, attr, value]) =>
  `${tag}${attributes.map((name) => `[${name}]`).join("")} ${attr}=${value}: ${morphed(tag, attributes, attr, value)}`)
 .join("\n")
"#;
    let answers = evaluated(&[BROWSER, &script, asked]);

    assert_eq!(
        answers.lines().collect::<Vec<_>>(),
        [
            "DETAILS open=: false",
            "DETAILS open=null: false",
            "DETAILS class=lane: true",
            "DIALOG open=: false",
            "DIALOG open=null: false",
            "SECTION open=null: true",
            "BUTTON[aria-controls] aria-expanded=false: false",
            "BUTTON[aria-controls] class=fold: true",
            "BUTTON aria-expanded=false: true",
            "TR[data-fold-of] hidden=: false",
            "TR[data-fold-of] hidden=null: false",
            "TR hidden=: true",
        ]
    );
}

/// `docs/specs/design_system.md`, Confirming; `docs/specs/runs.md`, Run,
/// Capturing: the capture says what came of it in its own dialog.
#[test_log::test(tokio::test)]
async fn going_ahead_closes_the_confirmation_but_not_a_dialog_that_answers_within_and_turning_it_down_is_left_to_the_browser()
 {
    let script = served_script().await;

    // A form in a dialog as the browser gives it to a `submit` listener:
    // https://html.spec.whatwg.org/multipage/forms.html#dom-fs-method
    let asked = r#"
var submitted = [];
document.addEventListener = (type, listener) => { if (type === "submit") submitted.push(listener); };
"#;
    let submit = r##"
const dialog = (holds = []) => ({
  open: true,
  close() { this.open = false; },
  querySelector: (selector) => (holds.includes(selector) ? {} : null),
});
const ahead = dialog();
const down = dialog();
const making = dialog();
const capturing = dialog(["#capture-said"]);
const form = (method, within, attributes, target = null) => ({
  method,
  closest: (selector) => (selector === "dialog" ? within : null),
  hasAttribute: (name) => attributes.includes(name),
  getAttribute: (name) => (name === "hx-target" ? target : null),
});
for (const listener of submitted) {
  listener({ target: form("post", ahead, [], "#benchmark-run-said") });
  listener({ target: form("dialog", down, []) });
  listener({ target: form("get", null, []) });
  listener({ target: form("post", making, ["data-new"], "this") });
  listener({ target: form("post", capturing, [], "#capture-said") });
}
`ahead: ${ahead.open}, down: ${down.open}, making: ${making.open}, capturing: ${capturing.open}`
"##;
    let answer = evaluated(&[BROWSER, asked, &script, submit]);

    assert_eq!(
        answer,
        "ahead: false, down: true, making: true, capturing: true"
    );
}

/// Stand-ins for the page's `#tip` and its triggers, and for the events the
/// browser sends the document: `node(attributes, ...children)` makes an
/// element, `page(...)` puts elements on the page, `fire(type, event)` sends
/// an event to the listeners the script added, and `elapse()` runs the
/// timers it set. `shown()` is what the tip shows, or `-` while it is hidden.
const TIPS: &str = r#"
var listeners = {};
function listen(type, listener) { (listeners[type] = listeners[type] || []).push(listener); }
document.addEventListener = listen;
document.body = { addEventListener: listen, getAttribute() { return null; } };
window.addEventListener = (type, listener) => listen(`window ${type}`, listener);
window.innerWidth = 1000;
function fire(type, event) { for (const listener of listeners[type] || []) listener(event); }
var timers = [];
function setTimeout(callback) { timers.push(callback); return timers.length; }
function clearTimeout(id) { if (id) timers[id - 1] = null; }
function elapse() {
  const due = timers;
  timers = [];
  for (const callback of due) if (callback) callback();
}
var root = { parentElement: null, children: [] };
function adopt(parent, children) {
  for (const child of children) {
    child.parentElement = parent;
    parent.children.push(child);
  }
}
function page(...elements) { adopt(root, elements); }
function node(attributes, ...children) {
  const element = {
    attributes,
    children: [],
    text: "",
    focusVisible: false,
    clientWidth: 100,
    clientHeight: 20,
    scrollHeight: 20,
    rect: { left: 450, top: 300, width: 100, height: 20 },
    get scrollWidth() { return this.cut ? 300 : 100; },
    get textContent() { return this.text + this.children.map((child) => child.textContent).join(""); },
    get isConnected() {
      let at = this;
      while (at.parentElement) at = at.parentElement;
      return at === root;
    },
    hasAttribute(name) { return name in this.attributes; },
    getAttribute(name) { return name in this.attributes ? this.attributes[name] : null; },
    setAttribute(name, value) { this.attributes[name] = value; },
    removeAttribute(name) { delete this.attributes[name]; },
    contains(other) {
      for (let at = other; at; at = at.parentElement) if (at === this) return true;
      return false;
    },
    querySelectorAll(selector) {
      if (selector !== "*") return [];
      return this.children.flatMap((child) => [child, ...child.querySelectorAll("*")]);
    },
    matches(selector) { return selector === ":focus-visible" && this.focusVisible; },
    getBoundingClientRect() {
      const { left, top, width, height } = this.rect;
      return { left, top, width, height, right: left + width, bottom: top + height };
    },
    remove() {
      this.parentElement.children = this.parentElement.children.filter((child) => child !== this);
      this.parentElement = null;
    },
  };
  for (const child of children) {
    if (typeof child === "string") element.text += child;
    else adopt(element, [child]);
  }
  return element;
}
var tip = {
  id: "tip",
  open: false,
  parentElement: root,
  dataset: { tip: "" },
  classes: ["tooltip", "tooltip-open", "tooltip-top"],
  style: {
    properties: {},
    setProperty(name, value) { this.properties[name] = value; },
    removeProperty(name) { delete this.properties[name]; },
  },
  classList: {
    toggle(name, on) {
      tip.classes = tip.classes.filter((other) => other !== name);
      if (on) tip.classes.push(name);
    },
  },
  showPopover() { this.open = true; },
  hidePopover() { this.open = false; },
  contains(other) { return other === this; },
};
document.getElementById = (id) => (id === "tip" ? tip : null);
var drawn = { width: 120, height: 28 };
function getComputedStyle(element, pseudo) {
  if (element !== tip || pseudo !== "::before") return {};
  return {
    width: `${drawn.width}px`,
    height: `${drawn.height}px`,
    bottom: `${parseFloat(tip.style.height) + 8}px`,
  };
}
function shown() { return tip.open ? tip.dataset.tip : "-"; }
function point(from, to) {
  if (from) fire("pointerout", { target: from, relatedTarget: to });
  if (to) fire("pointerover", { target: to, relatedTarget: from });
}
"#;

/// What `asked` evaluates to, line by line, once the served script has run
/// on a page that `page` builds of the stand-ins in `TIPS`.
async fn with_tips(page: &str, asked: &str) -> Vec<String> {
    let script = served_script().await;
    evaluated(&[BROWSER, TIPS, page, &script, asked])
        .lines()
        .map(str::to_string)
        .collect()
}

#[test_log::test(tokio::test)]
async fn the_tip_shows_the_innermost_trigger_unless_a_cut_text_holds_it() {
    let page = r#"
var plain = node({ "data-tip": "plain" }, "x");
var cutAlone = node({ "data-tip-cut": "" }, "  The   whole\n title ");
var uncutAlone = node({ "data-tip-cut": "" }, "whole");
var inner = node({ "data-tip": "inner" });
var outer = node({ "data-tip": "outer" }, inner);
var inCut = node({ "data-tip": "in cut" });
var cutHead = node({ "data-tip-cut": "" }, "Head ", inCut);
var inUncut = node({ "data-tip": "in uncut" });
var uncutHead = node({ "data-tip-cut": "" }, "Head ", inUncut);
var hidden = node({}, "owner/widgets");
var repo = node({ "data-tip": "owner/widgets", "data-tip-cut": "" }, hidden);
var bare = node({});
cutAlone.cut = true;
cutHead.cut = true;
hidden.cut = true;
page(plain, cutAlone, uncutAlone, outer, cutHead, uncutHead, repo, bare);
"#;
    let asked = r#"
[plain, cutAlone, uncutAlone, inner, outer, inCut, inUncut, repo, hidden, bare]
  .map((target) => {
    point(null, target);
    const tip = shown();
    point(target, null);
    elapse();
    return tip;
  })
  .join("\n")
"#;

    let tips = with_tips(page, asked).await;

    assert_eq!(
        tips,
        [
            "plain",
            "The whole title",
            "-",
            "inner",
            "outer",
            "Head",
            "in uncut",
            "owner/widgets",
            "owner/widgets",
            "-",
        ]
    );
}

/// Two triggers, the first holding a link, for the steps of `asked`.
const TWO_TRIGGERS: &str = r#"
var link = node({});
var first = node({ "data-tip": "first" }, link);
var second = node({ "data-tip": "second" });
var elsewhere = node({});
page(first, second, elsewhere);
"#;

/// What the tip shows after each of `steps`, a JavaScript statement each.
async fn after_each(page: &str, steps: &[&str]) -> Vec<String> {
    let asked = steps
        .iter()
        .map(|step| format!("{step}\nsteps.push(shown());\n"))
        .collect::<String>();
    with_tips(
        page,
        &format!("const steps = [];\n{asked}steps.join(\"\\n\")"),
    )
    .await
}

#[test_log::test(tokio::test)]
async fn the_tip_hides_once_the_pointer_has_left_the_trigger_and_the_tip() {
    let steps = [
        ("point(null, first);", "first"),
        ("point(first, link);", "first"),
        ("point(link, first); elapse();", "first"),
        ("point(first, tip); elapse();", "first"),
        ("point(tip, link); elapse();", "first"),
        ("point(link, elsewhere);", "first"),
        ("point(elsewhere, first); elapse();", "first"),
        ("point(first, elsewhere); elapse();", "-"),
    ];

    let tips = after_each(TWO_TRIGGERS, &steps.map(|(step, _)| step)).await;

    assert_eq!(tips, steps.map(|(_, tip)| tip));
}

#[test_log::test(tokio::test)]
async fn one_tip_shows_on_hover_or_keyboard_focus_until_focus_moves() {
    let steps = [
        (r#"fire("focusin", { target: link });"#, "-"),
        (
            r#"link.focusVisible = true; fire("focusin", { target: link });"#,
            "first",
        ),
        ("point(null, second);", "second"),
        (r#"fire("focusout", { target: link });"#, "second"),
        (r#"fire("focusin", { target: link });"#, "first"),
        (
            "point(null, elsewhere); point(elsewhere, null); elapse();",
            "first",
        ),
        (r#"fire("focusout", { target: link });"#, "-"),
    ];

    let tips = after_each(TWO_TRIGGERS, &steps.map(|(step, _)| step)).await;

    assert_eq!(tips, steps.map(|(_, tip)| tip));
}

#[test_log::test(tokio::test)]
async fn keyboard_focus_on_what_holds_a_cut_text_shows_it_whole_until_focus_moves() {
    let page = r##"
var head = node({ "data-tip-cut": "" }, "#4: Old idea — finished on Oct 1");
var summary = node({}, head);
var uncutHead = node({ "data-tip-cut": "" }, "#5: Short");
var uncutSummary = node({}, uncutHead);
head.cut = true;
summary.focusVisible = true;
uncutSummary.focusVisible = true;
page(summary, uncutSummary);
"##;
    let tips = after_each(
        page,
        &[
            r#"fire("focusin", { target: summary });"#,
            r#"fire("focusout", { target: summary });"#,
            r#"fire("focusin", { target: uncutSummary });"#,
        ],
    )
    .await;

    assert_eq!(tips, ["#4: Old idea — finished on Oct 1", "-", "-"]);
}

#[test_log::test(tokio::test)]
async fn escape_hides_the_tip_until_the_pointer_leaves_the_trigger_or_focus_moves() {
    let escape = r#"fire("keydown", { key: "Escape" });"#;
    let steps = [
        ("point(null, first);", "first"),
        (r#"fire("keydown", { key: "Enter" });"#, "first"),
        (escape, "-"),
        ("point(first, link);", "-"),
        ("point(link, first);", "-"),
        ("point(first, elsewhere); elapse();", "-"),
        ("point(elsewhere, first);", "first"),
        (escape, "-"),
        (
            r#"link.focusVisible = true; fire("focusin", { target: link });"#,
            "first",
        ),
        (escape, "-"),
        (r#"fire("focusout", { target: link });"#, "-"),
        ("point(first, elsewhere); point(elsewhere, first);", "first"),
    ];

    let tips = after_each(TWO_TRIGGERS, &steps.map(|(step, _)| step)).await;

    assert_eq!(tips, steps.map(|(_, tip)| tip));
}

#[test_log::test(tokio::test)]
async fn a_trigger_an_update_brings_shows_its_tip_and_one_it_takes_away_hides_it() {
    let update = r#"fire("htmx:sse:after:message", { detail: { message: { id: "" } } });"#;
    let steps = [
        (
            r#"var card = node({ "data-tip-cut": "" }, "A long title"); card.cut = true; page(card);"#,
            "-",
        ),
        ("point(null, card);", "A long title"),
        (r#"card.text = "A longer title";"#, "A long title"),
        (update, "A longer title"),
        ("card.cut = false;", "A longer title"),
        (update, "-"),
        (
            "card.cut = true; point(card, null); point(null, card);",
            "A longer title",
        ),
        ("card.remove();", "A longer title"),
        (update, "-"),
    ];

    let tips = after_each("page();", &steps.map(|(step, _)| step)).await;

    assert_eq!(tips, steps.map(|(_, tip)| tip));
}

#[test_log::test(tokio::test)]
async fn a_trigger_whose_tip_is_not_in_its_text_or_label_is_described_by_it_while_it_shows() {
    let page = r##"
var time = node({ "data-tip": "Oct 23, 2026, 11:00:00 UTC" }, "1 hour ago");
var stamp = node({ "data-tip": "2026-10-13 10:09:11 UTC" }, "0:03");
var github = node({ "data-tip": "#12 on GitHub", "aria-label": "#12 on GitHub" });
var title = node({ "data-tip-cut": "" }, "A long title");
title.cut = true;
page(time, stamp, github, title);
const described = () =>
  [time, stamp, github, title].map((trigger) => trigger.getAttribute("aria-describedby") ?? "-").join(" ");
"##;
    let tips = after_each(
        page,
        &[
            "point(null, time); steps.push(described());",
            "point(time, stamp); steps.push(described());",
            "point(stamp, github); steps.push(described());",
            "point(github, title); steps.push(described());",
            "point(title, time); point(time, null); elapse(); steps.push(described());",
        ],
    )
    .await;

    assert_eq!(
        tips.iter().step_by(2).collect::<Vec<_>>(),
        ["tip - - -", "- tip - -", "- - - -", "- - - -", "- - - -",]
    );
}

#[test_log::test(tokio::test)]
async fn the_tip_lies_over_its_trigger_above_it_or_below_where_there_is_no_room_inside_the_viewport()
 {
    let page = r#"
var trigger = node({ "data-tip": "tip" });
page(trigger);
const placed = () =>
  [tip.style.left, tip.style.top, tip.style.width, tip.style.height, tip.classes.join(" "),
   tip.style.properties["--tt-trans"] || "centred"].join(" ");
"#;
    let tips = after_each(
        page,
        &[
            "point(null, trigger); steps.push(placed());",
            "trigger.rect.top = 35; fire(\"scroll\", {}); steps.push(placed());",
            "trigger.rect.top = 36; trigger.rect.left = 0; fire(\"window resize\", {}); steps.push(placed());",
            "trigger.rect.left = 900; fire(\"scroll\", {}); steps.push(placed());",
        ],
    )
    .await;

    assert_eq!(
        tips.iter().step_by(2).collect::<Vec<_>>(),
        [
            "450px 300px 100px 20px tooltip tooltip-open tooltip-top centred",
            "450px 35px 100px 20px tooltip tooltip-open tooltip-bottom centred",
            "0px 36px 100px 20px tooltip tooltip-open tooltip-top calc(-50% + 18px)",
            "900px 36px 100px 20px tooltip tooltip-open tooltip-top calc(-50% + -18px)",
        ]
    );
}

/// The moment the clock in `TIMES` stops at.
const NOW: &str = "2026-10-23T12:00:00Z";

/// Stand-ins for `Intl`'s formatters that echo the language, the options and
/// what they format, a clock stopped at `now`, and the page's `<time>`s,
/// which `times` holds by the `datetime` or `data-since` they carry.
const TIMES: &str = r#"
Date.now = () => now;
document.documentElement.lang = "en";
const echoed = (name, lang, options) =>
  `${name}(${lang}, ${JSON.stringify(options)})`;
Intl = {
  RelativeTimeFormat: function (lang, options) {
    this.format = (value, unit) => `${echoed("relative", lang, options)}: ${value} ${unit}`;
  },
  DateTimeFormat: function (lang, options) {
    this.format = (at) =>
      `${echoed("date", lang, options)}: ${new Date(at).toISOString()}`;
  },
  DurationFormat: function (lang, options) {
    this.format = (duration) =>
      `${echoed("duration", lang, options)}: ${JSON.stringify(duration)}`;
  },
};
var times = [];
function time(attributes) {
  const shown = { textContent: "", dataset: {}, ...attributes };
  shown.hasAttribute = (name) => name === "title" ? "title" in shown : name.startsWith("data-") && name.slice(5) in shown.dataset;
  shown.removeAttribute = (name) => {
    if (name === "title") delete shown.title;
    else if (name.startsWith("data-")) delete shown.dataset[name.slice(5)];
  };
  times.push(shown);
  return shown;
}
document.querySelectorAll = (selector) => {
  if (selector.includes("data-since")) return times.filter((time) => time.dataset.since);
  if (selector.includes(":not(")) {
    return times.filter((time) => time.dateTime && !time.dateTime.startsWith("PT"));
  }
  if (selector.includes('datetime^="PT"')) {
    return times.filter((time) => time.dateTime && time.dateTime.startsWith("PT"));
  }
  return [];
};
"#;

/// What `asked` evaluates to, line by line, once the served script has run
/// on a page holding the times `page` adds, as of `NOW`.
async fn on_the_page(page: &str, asked: &str) -> Vec<String> {
    let script = served_script().await;
    let clock = format!("var now = Date.parse({NOW:?});");
    evaluated(&[BROWSER, &clock, TIMES, page, &script, asked])
        .lines()
        .map(str::to_string)
        .collect()
}

/// How the stand-in for `Intl` in `TIMES` echoes a time formatted as the
/// absolute time.
const ABSOLUTE: &str = r#"date(en, {"year":"numeric","month":"short","day":"numeric","hour":"2-digit","minute":"2-digit","second":"2-digit","timeZoneName":"short"})"#;

/// A `<time>` for each of `datetimes`.
fn times(datetimes: &[&str]) -> String {
    datetimes
        .iter()
        .map(|at| format!("time({{ dateTime: {at:?} }});\n"))
        .collect()
}

#[test_log::test(tokio::test)]
async fn a_past_time_reads_in_words_rounded_down_then_as_its_date_past_30_days_with_its_absolute_time_in_the_tip()
 {
    let datetimes = [
        "2026-10-23T11:59:01Z",
        "2026-10-23T11:59:00Z",
        "2026-10-23T11:00:01Z",
        "2026-10-23T11:00:00Z",
        "2026-10-22T12:01:00Z",
        "2026-10-22T12:00:00Z",
        "2026-10-21T13:00:00Z",
        "2026-09-23T12:00:00Z",
        "2026-09-23T11:59:59Z",
        "2025-09-12T10:00:00Z",
    ];

    let shown = on_the_page(
        &times(&datetimes),
        r#"times.map((time) => `${time.textContent} | ${time.dataset.tip} | ${time.hasAttribute("title")}`).join("\n")"#,
    )
    .await;

    let relative = r#"relative(en, {"numeric":"always"})"#;
    let date_this_year = r#"date(en, {"month":"short","day":"numeric"})"#;
    let date_with_year = r#"date(en, {"year":"numeric","month":"short","day":"numeric"})"#;
    let words: Vec<&str> = shown
        .iter()
        .map(|line| line.split(" | ").next().unwrap())
        .collect();
    assert_eq!(
        words,
        [
            "just now".to_string(),
            format!("{relative}: -1 minute"),
            format!("{relative}: -59 minute"),
            format!("{relative}: -1 hour"),
            format!("{relative}: -23 hour"),
            format!("{relative}: -1 day"),
            format!("{relative}: -1 day"),
            format!("{relative}: -30 day"),
            format!("on {date_this_year}: 2026-09-23T11:59:59.000Z"),
            format!("on {date_with_year}: 2025-09-12T10:00:00.000Z"),
        ]
    );
    let tips: Vec<&str> = shown
        .iter()
        .map(|line| line.split(" | ").nth(1).unwrap())
        .collect();
    assert_eq!(
        tips,
        [
            "2026-10-23T11:59:01.000Z",
            "2026-10-23T11:59:00.000Z",
            "2026-10-23T11:00:01.000Z",
            "2026-10-23T11:00:00.000Z",
            "2026-10-22T12:01:00.000Z",
            "2026-10-22T12:00:00.000Z",
            "2026-10-21T13:00:00.000Z",
            "2026-09-23T12:00:00.000Z",
            "2026-09-23T11:59:59.000Z",
            "2025-09-12T10:00:00.000Z",
        ]
        .map(|at| format!("{ABSOLUTE}: {at}")),
        "the absolute time in the page's tooltip"
    );
    assert!(
        shown.iter().all(|line| line.ends_with(" | false")),
        "no title: {shown:?}"
    );
}

#[test_log::test(tokio::test)]
async fn a_future_time_reads_as_its_absolute_time_with_no_tip() {
    let page = r#"time({ dateTime: "2026-10-23T12:00:01Z", dataset: { tip: "stale" } });"#;

    let shown = on_the_page(
        page,
        r#"times.map((time) => `${time.textContent} | ${time.hasAttribute("data-tip")}`).join("\n")"#,
    )
    .await;

    assert_eq!(
        shown,
        [format!("{ABSOLUTE}: 2026-10-23T12:00:01.000Z | false")]
    );
}

#[test_log::test(tokio::test)]
async fn a_runs_duration_reads_its_two_largest_units_that_are_not_zero() {
    let page = times(&["PT45S", "PT103S", "PT10927S", "PT86709S", "PT0S"])
        + r#"time({ dataset: { since: "2026-10-23T08:57:53Z" } });"#;

    let shown = on_the_page(&page, r#"times.map((time) => time.textContent).join("\n")"#).await;

    let narrow = r#"duration(en, {"style":"narrow"})"#;
    let zero = r#"duration(en, {"style":"narrow","secondsDisplay":"always"})"#;
    assert_eq!(
        shown,
        [
            format!(r#"{narrow}: {{"seconds":45}}"#),
            format!(r#"{narrow}: {{"minutes":1,"seconds":43}}"#),
            format!(r#"{narrow}: {{"hours":3,"minutes":2}}"#),
            format!(r#"{narrow}: {{"days":1,"minutes":5}}"#),
            format!(r#"{zero}: {{"seconds":0}}"#),
            format!(r#"{narrow}: {{"hours":3,"minutes":2}}"#),
        ]
    );
}

/// Stand-ins for a page with filters (`docs/specs/design_system.md`,
/// Filters) and folds: `form` holds the fields `field(tagName, name,
/// options)` makes, a select with `options` the values of its options,
/// `fire(type, event)` sends an event to the listeners the script added,
/// `event(target, more)` makes one, `fold(id, open)` puts a fold on the page
/// in place of any with its id, and `source(id, values)` puts an element
/// holding options of `values` there. `applied()` is each address the page
/// took and each stream it restarted since it was last asked, or `-` for
/// none, and `listed(select)` is its options' values, its chosen one marked
/// `*`.
const FILTERS: &str = r#"
var listeners = {};
function listen(type, listener) { (listeners[type] = listeners[type] || []).push(listener); }
document.addEventListener = listen;
document.body = { addEventListener: listen, getAttribute() { return null; } };
function fire(type, event) { for (const listener of listeners[type] || []) listener(event); }
function event(target, more) {
  return Object.assign({ target, prevented: false, preventDefault() { this.prevented = true; } }, more);
}
var location = { pathname: "/runs", search: "" };
var addresses = [];
var history = { replaceState(state, unused, url) { addresses.push(url); } };
var swaps = [];
htmx.swap = (ctx) => { swaps.push(ctx); return Promise.resolve(); };
function element(tagName, attributes) {
  return {
    tagName,
    attributes,
    closest() { return null; },
    getAttribute(name) { return name in this.attributes ? this.attributes[name] : null; },
    setAttribute(name, value) { this.attributes[name] = value; },
    cloneNode() { return element(this.tagName, Object.assign({}, this.attributes)); },
    get outerHTML() { return `${this.tagName} ${JSON.stringify(this.attributes)}`; },
  };
}
var stream = element("DIV", { id: "stream", "hx-sse:connect": "/events/runs" });
var form = element("FORM", { "data-filters": "/events/runs" });
form.elements = [];
form.closest = (selector) => (selector === "form[data-filters]" ? form : null);
function field(tagName, name, options) {
  const field = element(tagName, {});
  Object.assign(field, { name, value: "", form, closest: form.closest });
  if (options) {
    field.options = options.map((value) => new Option(value, value));
    field.replaceChildren = (...options) => { field.options = options; };
  }
  form.elements.push(field);
  return field;
}
function Option(text, value) {
  return { text, value };
}
var sources = {};
function source(id, values) {
  sources[id] = { id, options: values.map((value) => new Option("", value)) };
  sources[id].querySelectorAll = (selector) => (selector === "option" ? sources[id].options : []);
}
document.querySelectorAll = (selector) =>
  selector === "select[data-options]"
    ? form.elements.filter((field) => field.tagName === "SELECT" && "data-options" in field.attributes)
    : [];
function listed(select) {
  return select.options
    .map((option) => (option.value === select.value ? `*${option.value}` : option.value))
    .join(",");
}
var folds = {};
function fold(id, open) {
  folds[id] = Object.assign(element("DETAILS", { id }), { id, open });
  return folds[id];
}
document.getElementById = (id) => (id === "stream" ? stream : folds[id] || sources[id] || null);
function applied() {
  const shown = addresses
    .concat(swaps.map((swap) => `${swap.swap} ${swap.target.attributes.id}: ${swap.text}`))
    .join(" | ");
  addresses = [];
  swaps = [];
  return shown || "-";
}
"#;

/// What `asked` evaluates to, line by line, once the served script has run
/// on a page that `page` builds of the stand-ins in `FILTERS`.
async fn with_filters(page: &str, asked: &str) -> Vec<String> {
    let script = served_script().await;
    evaluated(&[BROWSER, FILTERS, page, &script, asked])
        .lines()
        .map(str::to_string)
        .collect()
}

/// What each of `steps`, a JavaScript statement each, applied, on the runs
/// page's filters.
async fn applied_after_each(steps: &[&str]) -> Vec<String> {
    let page = r#"
var item = field("INPUT", "item");
var stage = field("SELECT", "stage");
var since = field("INPUT", "since");
"#;
    let asked = steps
        .iter()
        .map(|step| format!("{step}\nsteps.push(applied());\n"))
        .collect::<String>();
    with_filters(
        page,
        &format!("const steps = [];\n{asked}steps.join(\"\\n\")"),
    )
    .await
}

/// The stream `stream` restarted at.
fn restarted(stream: &str) -> String {
    format!(r#"outerHTML stream: DIV {{"id":"stream","hx-sse:connect":"{stream}"}}"#)
}

#[test_log::test(tokio::test)]
async fn a_chosen_select_applies_at_once_to_the_address_and_the_stream() {
    let applied = applied_after_each(&[
        r#"stage.value = "triage"; fire("change", event(stage));"#,
        r#"stage.value = ""; fire("change", event(stage));"#,
    ])
    .await;

    assert_eq!(
        applied,
        [
            format!(
                "/runs?stage=triage | {}",
                restarted("/events/runs?stage=triage")
            ),
            format!("/runs | {}", restarted("/events/runs")),
        ]
    );
}

#[test_log::test(tokio::test)]
async fn a_typed_filter_applies_on_enter_alone_with_the_others_set() {
    let applied = applied_after_each(&[
        r#"stage.value = "triage"; fire("change", event(stage));"#,
        r#"item.value = " 5 "; fire("input", event(item)); fire("keydown", event(item, { key: "5" }));"#,
        r#"const enter = event(item, { key: "Enter" }); fire("keydown", enter); steps.push(enter.prevented);"#,
        r#"since.value = "2h ago"; fire("keydown", event(since, { key: "Enter" }));"#,
    ])
    .await;

    assert_eq!(
        applied,
        [
            format!(
                "/runs?stage=triage | {}",
                restarted("/events/runs?stage=triage")
            ),
            "-".to_string(),
            "true".to_string(),
            format!(
                "/runs?item=5&stage=triage | {}",
                restarted("/events/runs?item=5&stage=triage")
            ),
            format!(
                "/runs?item=5&stage=triage&since=2h%20ago | {}",
                restarted("/events/runs?item=5&stage=triage&since=2h%20ago")
            ),
        ]
    );
}

/// `docs/specs/design_system.md`, Tables, Pages, Links: a page link applies
/// to the address and the stream as a filter does, a link opened in a new tab
/// is left to the browser, and changing a filter goes back to the first page.
#[test_log::test(tokio::test)]
async fn a_page_link_applies_to_the_address_and_the_stream_and_a_filter_then_drops_the_page() {
    let applied = applied_after_each(&[
        r#"
var link = element("A", { href: "/runs?stage=triage&page=2", "data-stream": "/events/runs?stage=triage&page=2" });
link.closest = (selector) => (selector === "nav.pages a[data-stream]" ? link : null);
const click = event(link, { button: 0 });
fire("click", click);
steps.push(click.prevented);"#,
        r#"const opened = event(link, { button: 0, ctrlKey: true }); fire("click", opened); steps.push(opened.prevented);"#,
        r#"stage.value = "design"; fire("change", event(stage));"#,
    ])
    .await;

    assert_eq!(
        applied,
        [
            "true".to_string(),
            format!(
                "/runs?stage=triage&page=2 | {}",
                restarted("/events/runs?stage=triage&page=2")
            ),
            "false".to_string(),
            "-".to_string(),
            format!(
                "/runs?stage=design | {}",
                restarted("/events/runs?stage=design")
            ),
        ]
    );
}

/// The Benchmark page's tabs, Runs open, for `applied_after_each`'s steps:
/// `tabs()` is each tab's href, the open one marked `*`, and each panel's
/// id, the hidden one marked `-`.
const TABS: &str = r#"
var tablist = element("DIV", { role: "tablist" });
function tab(href, panel, open) {
  const tab = element("A", { href, "aria-controls": panel, "aria-selected": String(open) });
  tab.classList = { toggle(name, on) { tab.active = on; } };
  tab.active = open;
  tab.closest = (selector) =>
    selector === "[role=tablist] a[role=tab][aria-controls]" ? tab : selector === "[role=tablist]" ? tablist : null;
  folds[panel] = { id: panel, hidden: !open };
  return tab;
}
var runs = tab("/benchmarks/board/runs?runs-page=2", "runs-panel", true);
var tasks = tab("/benchmarks/board/tasks?runs-page=2", "tasks-panel", false);
tablist.querySelectorAll = () => [runs, tasks];
const others = document.querySelectorAll;
document.querySelectorAll = (selector) =>
  selector === "[role=tablist] a[role=tab][aria-controls]" ? [runs, tasks] : others(selector);
function tabs() {
  return [runs, tasks]
    .map((tab) => {
      const open = tab.active && tab.attributes["aria-selected"] === "true";
      const panel = folds[tab.attributes["aria-controls"]];
      return `${open ? "*" : ""}${tab.attributes.href} ${panel.hidden ? "-" : ""}${panel.id}`;
    })
    .join(", ");
}
"#;

/// `docs/specs/design_system.md`, Tabs; `docs/specs/web/design_system.md`, A
/// tab: a tab opens in place, its panel shown and the other's hidden, and the
/// address becomes its with no reload; one the reader opens elsewhere is left
/// to the browser.
#[test_log::test(tokio::test)]
async fn a_tab_opens_in_place_and_the_address_becomes_its() {
    let applied = applied_after_each(&[
        TABS,
        r#"const opened = event(tasks, { button: 0, ctrlKey: true }); fire("click", opened); steps.push(opened.prevented); steps.push(tabs());"#,
        r#"const click = event(tasks, { button: 0 }); fire("click", click); steps.push(click.prevented); steps.push(tabs());"#,
    ])
    .await;

    assert_eq!(
        applied,
        [
            "-",
            "false",
            "*/benchmarks/board/runs?runs-page=2 runs-panel, /benchmarks/board/tasks?runs-page=2 -tasks-panel",
            "-",
            "true",
            "/benchmarks/board/runs?runs-page=2 -runs-panel, */benchmarks/board/tasks?runs-page=2 tasks-panel",
            "/benchmarks/board/tasks?runs-page=2",
        ]
    );
}

/// `docs/specs/design_system.md`, Tabs, Links: a tab's link carries the
/// pages a page link applied.
#[test_log::test(tokio::test)]
async fn a_page_link_carries_its_pages_into_the_tab_links() {
    let applied = applied_after_each(&[
        TABS,
        r#"
var link = element("A", { href: "/benchmarks/board/runs?runs-page=3&tasks-page=2", "data-stream": "/events/benchmarks/board?runs-page=3&tasks-page=2" });
link.closest = (selector) => (selector === "nav.pages a[data-stream]" ? link : null);
fire("click", event(link, { button: 0 }));
steps.push(tabs());"#,
        r#"
link.attributes.href = "/benchmarks/board/runs";
link.attributes["data-stream"] = "/events/benchmarks/board";
fire("click", event(link, { button: 0 }));
steps.push(tabs());"#,
    ])
    .await;

    assert_eq!(
        applied,
        [
            "-".to_string(),
            "*/benchmarks/board/runs?runs-page=3&tasks-page=2 runs-panel, /benchmarks/board/tasks?runs-page=3&tasks-page=2 -tasks-panel".to_string(),
            format!(
                "/benchmarks/board/runs?runs-page=3&tasks-page=2 | {}",
                restarted("/events/benchmarks/board?runs-page=3&tasks-page=2")
            ),
            "*/benchmarks/board/runs runs-panel, /benchmarks/board/tasks -tasks-panel".to_string(),
            format!(
                "/benchmarks/board/runs | {}",
                restarted("/events/benchmarks/board")
            ),
        ]
    );
}

/// `docs/specs/web/design_system.md`, Filters: a search the reader clears by
/// its own button or by Escape, which fire `search` and no Enter, unsets its
/// filter; a `search` the reader's Enter already applied applies nothing more.
#[test_log::test(tokio::test)]
async fn a_search_cleared_by_its_button_unsets_its_filter_once() {
    let applied = applied_after_each(&[
        r#"location.search = "?item=5&stage=triage"; stage.value = "triage"; fire("search", event(item));"#,
        r#"location.search = "?stage=triage"; fire("keydown", event(item, { key: "Enter" })); fire("search", event(item));"#,
        r#"item.value = "6"; fire("search", event(item));"#,
    ])
    .await;

    assert_eq!(
        applied,
        [
            format!(
                "/runs?stage=triage | {}",
                restarted("/events/runs?stage=triage")
            ),
            format!(
                "/runs?stage=triage | {}",
                restarted("/events/runs?stage=triage")
            ),
            "-".to_string(),
        ]
    );
}

/// `docs/specs/board.md`, Filtering by person, The people, and
/// `docs/specs/design_system.md`, Filters, Kept.
#[test_log::test(tokio::test)]
async fn a_select_takes_its_sources_options_after_a_message_and_keeps_its_choice() {
    let page = r#"
var person = field("SELECT", "person", ["", "bob", "carol"]);
person.setAttribute("data-options", "people");
person.value = "bob";
var stage = field("SELECT", "stage", ["", "triage"]);
source("people", ["bob", "carol"]);
"#;
    let asked = r#"
const steps = [];
const message = () => fire("htmx:sse:after:message", { detail: { message: { id: "" } } });
source("people", ["alice", "bob", "dave"]); message();
steps.push(`${listed(person)} ${listed(stage)} ${applied()}`);
source("people", ["alice", "dave"]); message();
steps.push(`${listed(person)} ${applied()}`);
person.value = ""; source("people", ["dave"]); message();
steps.push(`${listed(person)} ${applied()}`);
steps.join("\n")
"#;

    let steps = with_filters(page, asked).await;

    assert_eq!(
        steps,
        [
            ",alice,*bob,dave *,triage -",
            ",alice,*bob,dave -",
            "*,dave -",
        ]
    );
}

/// `docs/specs/design_system.md`, Filters: Opened and closed.
#[test_log::test(tokio::test)]
async fn a_fold_the_reader_opened_or_closed_stays_so_when_an_update_brings_it_back() {
    let page = r#"
var opened = fold("lane-1", false);
var closed = fold("lane-2", true);
var untouched = fold("lane-3", false);
"#;
    let asked = r#"
opened.open = true; fire("toggle", event(opened));
closed.open = false; fire("toggle", event(closed));
fold("lane-1", false); fold("lane-2", true); fold("lane-3", true);
fire("htmx:sse:after:message", { detail: { message: { id: "" } } });
["lane-1", "lane-2", "lane-3"].map((id) => `${id}: ${document.getElementById(id).open}`).join("\n")
"#;

    let folds = with_filters(page, asked).await;

    assert_eq!(folds, ["lane-1: true", "lane-2: false", "lane-3: true"]);
}

/// `docs/specs/design_system.md`, Making: a "New" dialog closes once
/// Linewright has made the thing, and opens again as it starts however it
/// was closed.
#[test_log::test(tokio::test)]
async fn a_new_dialog_closes_once_made_and_opens_again_with_its_form_as_it_started() {
    let page = r#"
var dialog = Object.assign(element("DIALOG", {}), {
  open: true,
  close() { this.open = false; fire("close", event(this)); },
});
var making = Object.assign(element("FORM", { "data-new": "" }), { method: "post", innerHTML: "as it starts" });
making.hasAttribute = (name) => name in making.attributes;
making.closest = (selector) => (selector === "dialog" ? dialog : null);
making.querySelectorAll = () => [];
dialog.querySelector = (selector) => (selector === "form[data-new]" ? making : null);
var opening = element("BUTTON", { command: "show-modal", commandfor: "new-benchmark" });
opening.closest = (selector) => (selector === "button[command=show-modal]" ? opening : null);
const others = document.querySelectorAll;
document.querySelectorAll = (selector) =>
  selector === "dialog form[data-new]" ? [making] : others(selector);
const byId = document.getElementById;
document.getElementById = (id) => (id === "new-benchmark" ? dialog : byId(id));
const open = () => { fire("click", event(opening)); dialog.open = true; };
const shown = () => `${dialog.open ? "open" : "closed"}: ${making.innerHTML}`;
"#;
    let asked = r#"
const steps = [];
making.innerHTML = "refused"; fire("submit", event(making));
steps.push(shown());
fire("made", event(making));
steps.push(shown());
open(); making.innerHTML = "typed"; dialog.close();
steps.push(shown());
making.innerHTML = "answered once closed"; open();
steps.push(shown());
steps.join("\n")
"#;

    let steps = with_filters(page, asked).await;

    assert_eq!(
        steps,
        [
            "open: refused",
            "closed: as it starts",
            "closed: as it starts",
            "open: as it starts",
        ]
    );
}

/// `docs/specs/benchmarks.md`, New task: a "New" dialog starts afresh
/// with the regions of the page's stream it holds as the stream last sent
/// them, New task's run chosen as it starts and its name suggested for it.
#[test_log::test(tokio::test)]
async fn a_new_dialog_starts_afresh_keeping_its_regions_as_the_stream_last_sent_them() {
    let page = r#"
var dialog = Object.assign(element("DIALOG", {}), {
  open: true,
  close() { this.open = false; fire("close", event(this)); },
});
var making = Object.assign(element("FORM", { "data-new": "" }), { method: "post", elements: [] });
making.hasAttribute = (name) => name in making.attributes;
making.closest = (selector) => (selector === "dialog" ? dialog : null);
dialog.querySelector = (selector) => (selector === "form[data-new]" ? making : null);
var name = Object.assign(element("INPUT", {}), { name: "name", value: "" });
var run;
function hold(values, sent) {
  run = Object.assign(element("SELECT", { id: "new-task-run", "data-region": "" }), {
    id: "new-task-run", name: "run", form: making, selectedIndex: 0, sent,
  });
  run.options = values.map((value) =>
    Object.assign(element("OPTION", { "data-name": value.replace("/", "-") }), { value }));
  run.replaceWith = (other) => { run = other; making.elements = [run, name]; };
  making.elements = [run, name];
  return run;
}
var html = "as it starts";
Object.defineProperty(making, "innerHTML", {
  get: () => html,
  set: (value) => { html = value; hold(["1/implement-1"], "as the page started"); name.value = "1-implement-1"; },
});
hold(["1/implement-1"], "as the page started");
making.querySelectorAll = (selector) =>
  selector === "[data-region]" || selector === "select[name=run]" ? [run] : [];
making.querySelector = (selector) => (selector === '[id="new-task-run"]' ? run : null);
const others = document.querySelectorAll;
document.querySelectorAll = (selector) =>
  selector === "dialog form[data-new]" ? [making] : selector === "select[name=run]" ? [run] : others(selector);
const message = () => fire("htmx:sse:after:message", { detail: { message: { id: "" } } });
const shown = () => `${run.sent}: ${run.options[run.selectedIndex].value} ${name.value}`;
"#;
    let asked = r#"
const steps = [];
const sent = hold(["1/implement-2", "1/implement-1"], "as the stream sent it");
sent.selectedIndex = 1; fire("change", event(sent));
name.value = "mine";
steps.push(shown());
dialog.close();
steps.push(shown());
message();
steps.push(shown());
steps.join("\n")
"#;

    let steps = with_filters(page, asked).await;

    assert_eq!(
        steps,
        [
            "as the stream sent it: 1/implement-1 mine",
            "as the stream sent it: 1/implement-2 1-implement-2",
            "as the stream sent it: 1/implement-2 1-implement-2",
        ]
    );
}

/// `docs/specs/benchmarks.md`, Benchmark run, The table: a row opens to the
/// rows under it, and those to theirs, each staying as the reader left it
/// through updates; `docs/specs/design_system.md`, Tables: only its chevron
/// button opens it, and no other part of the row acts on a click.
#[test_log::test(tokio::test)]
async fn a_fold_row_opens_to_its_rows_by_its_button_alone_and_stays_as_the_reader_left_it_through_updates()
 {
    let page = r#"
var rows = {};
function row(id, under) {
  const tr = Object.assign(element("TR", under ? { id, "data-fold-of": under } : { id }), { id, hidden: !!under });
  const button = element("BUTTON", { "aria-expanded": "false", "aria-controls": `${id}-rows` });
  button.closest = (selector) =>
    selector === "tr[id]" ? tr : selector.startsWith("button[aria-expanded]") ? button : null;
  tr.chevron = element("svg", {});
  tr.chevron.closest = button.closest;
  tr.querySelector = (selector) => (selector.includes("button") ? button : null);
  tr.button = button;
  tr.cell = element("TD", {});
  tr.cell.closest = (selector) => (selector === "tr[id]" ? tr : null);
  tr.link = element("A", {});
  tr.link.closest = (selector) =>
    selector === "tr[id]" ? tr : selector === "a" ? tr.link : null;
  rows[id] = tr;
  return tr;
}
function rendered() {
  row("cheap"); row("cheap-task-a", "cheap"); row("cheap-task-a-trial-1", "cheap-task-a");
}
rendered();
const byId = document.getElementById;
document.getElementById = (id) => rows[id] || byId(id);
const others = document.querySelectorAll;
document.querySelectorAll = (selector) =>
  selector === "tr[data-fold-of]"
    ? Object.values(rows).filter((row) => "data-fold-of" in row.attributes)
    : others(selector);
const click = (id, on = "button") => fire("click", event(rows[id][on]));
const shown = () => Object.values(rows)
  .map((row) => `${row.id}${row.hidden ? " hidden" : ""} ${row.button.getAttribute("aria-expanded")}`)
  .join(", ");
"#;
    let asked = r#"
const steps = [];
click("cheap"); steps.push(shown());
click("cheap-task-a", "cell"); steps.push(shown());
click("cheap-task-a", "link"); steps.push(shown());
click("cheap-task-a", "chevron"); steps.push(shown());
click("cheap"); steps.push(shown());
rendered(); fire("htmx:sse:after:message", { detail: { message: { id: "" } } }); steps.push(shown());
click("cheap"); steps.push(shown());
steps.join("\n")
"#;

    let steps = with_filters(page, asked).await;

    assert_eq!(
        steps,
        [
            "cheap true, cheap-task-a false, cheap-task-a-trial-1 hidden false",
            "cheap true, cheap-task-a false, cheap-task-a-trial-1 hidden false",
            "cheap true, cheap-task-a false, cheap-task-a-trial-1 hidden false",
            "cheap true, cheap-task-a true, cheap-task-a-trial-1 false",
            "cheap false, cheap-task-a hidden true, cheap-task-a-trial-1 hidden false",
            "cheap false, cheap-task-a hidden true, cheap-task-a-trial-1 hidden false",
            "cheap true, cheap-task-a true, cheap-task-a-trial-1 false",
        ]
    );
}

/// `docs/specs/benchmarks.md`, Benchmark: Adding a task.
#[test_log::test(tokio::test)]
async fn choosing_a_run_to_capture_fills_in_its_suggested_name_unless_one_was_typed() {
    let page = r#"
var capture = element("FORM", {});
var run = Object.assign(element("SELECT", {}), { name: "run", form: capture, selectedIndex: 0 });
run.options = [["1/implement-2", "1-implement-2"], ["1/implement-1", "1-implement-1"]].map(
  ([value, name]) => Object.assign(element("OPTION", { "data-name": name }), { value }),
);
var name = Object.assign(element("INPUT", {}), { name: "name", form: capture, value: "1-implement-2" });
capture.elements = [run, name];
function choose(index) { run.selectedIndex = index; fire("change", event(run)); return name.value; }
"#;
    let asked = r#"
const steps = [];
steps.push(choose(1));
name.value = "mine";
steps.push(choose(0));
name.value = "";
steps.push(choose(0));
steps.join("\n")
"#;

    let names = with_filters(page, asked).await;

    assert_eq!(names, ["1-implement-1", "mine", "1-implement-2"]);
}

/// `docs/specs/benchmarks.md`, Benchmark: Adding a task.
#[test_log::test(tokio::test)]
async fn the_run_chosen_to_capture_stays_chosen_when_an_update_brings_new_runs() {
    let page = r#"
var capture = element("FORM", {});
var run = Object.assign(element("SELECT", {}), { name: "run", form: capture, selectedIndex: 0 });
function offer(values) {
  run.options = values.map((value) => Object.assign(element("OPTION", { "data-name": value }), { value }));
  run.selectedIndex = 0;
}
offer(["1/implement-2", "1/implement-1"]);
var name = Object.assign(element("INPUT", {}), { name: "name", form: capture, value: "" });
capture.elements = [run, name];
const others = document.querySelectorAll;
document.querySelectorAll = (selector) => (selector === "select[name=run]" ? [run] : others(selector));
const message = () => fire("htmx:sse:after:message", { detail: { message: { id: "" } } });
const chosen = () => run.options[run.selectedIndex].value;
"#;
    let asked = r#"
const steps = [];
run.selectedIndex = 1; fire("change", event(run));
offer(["1/implement-3", "1/implement-2", "1/implement-1"]); message();
steps.push(chosen());
offer(["1/implement-3", "1/implement-2"]); message();
steps.push(chosen());
steps.join("\n")
"#;

    let chosen = with_filters(page, asked).await;

    assert_eq!(chosen, ["1/implement-1", "1/implement-3"]);
}

/// `docs/specs/benchmarks.md`, New task, Fields, and
/// `docs/specs/design_system.md`, Making, A refusal: the name follows the
/// run an update chooses unless the reader typed one, and the run sent
/// stays chosen through updates.
#[test_log::test(tokio::test)]
async fn the_suggested_name_follows_the_run_an_update_chooses_and_the_run_sent_stays_chosen() {
    let page = r#"
var capture = element("FORM", {});
var run = Object.assign(element("SELECT", {}), { name: "run", form: capture, selectedIndex: 0 });
function offer(values) {
  run.options = values.map((value) => Object.assign(element("OPTION", { "data-name": value }), { value }));
  run.selectedIndex = 0;
}
offer(["1/implement-2"]);
var name = Object.assign(element("INPUT", {}), { name: "name", form: capture, value: "1/implement-2" });
capture.elements = [run, name];
capture.querySelectorAll = (selector) => (selector === "select[name=run]" ? [run] : []);
const others = document.querySelectorAll;
document.querySelectorAll = (selector) => (selector === "select[name=run]" ? [run] : others(selector));
const message = () => fire("htmx:sse:after:message", { detail: { message: { id: "" } } });
const shown = () => `${run.options[run.selectedIndex].value} ${name.value}`;
"#;
    let asked = r#"
const steps = [];
offer(["1/implement-3", "1/implement-2"]); message();
steps.push(shown());
name.value = "mine";
offer(["1/implement-4", "1/implement-3", "1/implement-2"]); message();
steps.push(shown());
name.value = "1/implement-4";
run.selectedIndex = 1; fire("submit", event(capture));
offer(["1/implement-5", "1/implement-4", "1/implement-3", "1/implement-2"]); message();
steps.push(shown());
steps.join("\n")
"#;

    let steps = with_filters(page, asked).await;

    assert_eq!(
        steps,
        [
            "1/implement-3 1/implement-3",
            "1/implement-4 mine",
            "1/implement-3 1/implement-3",
        ]
    );
}

/// `docs/specs/benchmarks.md`, New benchmark run, Fields: the dialog counts
/// its trials with the tasks the benchmark has as it opens.
#[test_log::test(tokio::test)]
async fn opening_the_launch_counts_its_trials_with_the_benchmarks_tasks_as_the_stream_last_sent_them()
 {
    let page = r#"
var offer = element("DIV", { id: "new-benchmark-run-offer", "data-tasks": "3" });
var count = { textContent: "" };
var launch = Object.assign(element("FORM", {}), {
  elements: { repetitions: { value: "2" } },
  querySelectorAll: (selector) => (selector === "fieldset.configuration" ? [{}] : []),
  querySelector: (selector) => (selector === ".trial-count" ? count : null),
});
var dialog = element("DIALOG", {});
dialog.querySelector = (selector) => (selector === "form.launch" ? launch : null);
var opening = element("BUTTON", { command: "show-modal", commandfor: "new-benchmark-run" });
opening.closest = (selector) => (selector === "button[command=show-modal]" ? opening : null);
const byId = document.getElementById;
document.getElementById = (id) =>
  id === "new-benchmark-run" ? dialog : id === "new-benchmark-run-offer" ? offer : byId(id);
"#;
    let asked = r#"
fire("click", event(opening));
count.textContent
"#;

    let counted = with_filters(page, asked).await;

    assert_eq!(
        counted,
        ["1 configuration × 3 tasks × 2 repetitions: 6 trials"]
    );
}

/// Stand-ins for the New benchmark run dialog: `launch`, its form, holding the
/// configuration `configuration`, numbered 1, with its choice of a file of
/// the brief `choice` and its boxes in `files`, each drawn from the page's
/// template with `__N__` and `__PATH__` to fill in; `texts`, the region of
/// the texts of the files of the brief, which `serve(texts)` updates;
/// `open()` opens the dialog, `choose(path)` chooses a file, and `boxes()`
/// is each box the configuration holds, as its label, its field's name, its
/// text and the text it started with.
const NEW_BENCHMARK_RUN: &str = r#"
var files = {
  boxes: [],
  append(box) { this.boxes.push(box); },
  querySelectorAll: (selector) =>
    selector === "textarea" ? files.boxes.map((box) => box.querySelector("textarea")) : [],
};
function drawn() {
  const label = Object.assign(element("LABEL", {}), {
    children: [], htmlFor: "new-benchmark-run-c__N__-briefs/__PATH__", textContent: "__PATH__",
  });
  const area = Object.assign(element("TEXTAREA", {}), {
    name: "c__N__.briefs/__PATH__", id: "new-benchmark-run-c__N__-briefs/__PATH__", value: "",
  });
  const started = Object.assign(element("INPUT", {}), { name: "c__N__.started/__PATH__", value: "" });
  const box = element("DIV", {});
  box.querySelectorAll = () => [label, area, started];
  box.querySelector = (selector) =>
    selector === "textarea" ? area : selector === "input[type=hidden]" ? started : null;
  box.remove = () => { files.boxes = files.boxes.filter((held) => held !== box); };
  box.label = label;
  return box;
}
var template = { content: { firstElementChild: { cloneNode: drawn } } };
var texts = element("DIV", { id: "brief-texts" });
function serve(given) { texts.attributes["data-texts"] = JSON.stringify(given); }
var count = { textContent: "" };
var launch = Object.assign(element("FORM", {}), {
  elements: { repetitions: { value: "1" } },
  querySelectorAll: (selector) => (selector === "fieldset.configuration" ? [configuration] : []),
  querySelector: (selector) => (selector === ".trial-count" ? count : null),
});
var configuration = Object.assign(element("FIELDSET", {}), {
  dataset: { n: "1" },
  querySelector: (selector) => (selector === ".files" ? files : null),
});
var choice = Object.assign(element("SELECT", {}), { value: "" });
choice.classList = { contains: (name) => name === "brief-file" };
choice.closest = (selector) =>
  selector === "form.launch" ? launch : selector === "fieldset.configuration" ? configuration : null;
var dialog = element("DIALOG", {});
dialog.querySelector = (selector) => (selector === "form.launch" ? launch : null);
var opening = element("BUTTON", { command: "show-modal", commandfor: "new-benchmark-run" });
opening.closest = (selector) => (selector === "button[command=show-modal]" ? opening : null);
const byId = document.getElementById;
document.getElementById = (id) =>
  id === "new-benchmark-run" ? dialog
    : id === "brief-texts" ? texts
    : id === "brief-file-template" ? template
    : byId(id);
const open = () => fire("click", event(opening));
const choose = (path) => { choice.value = path; fire("change", event(choice)); };
const boxes = () => files.boxes.map((box) => {
  const area = box.querySelector("textarea");
  const started = box.querySelector("input[type=hidden]");
  return `${box.label.textContent} ${area.name}: ${area.value} (started ${started.value})`;
}).join(" | ");
"#;

/// `docs/specs/benchmarks.md`, New benchmark run, Files of the brief: choosing
/// a file opens a box for its text, which starts as the text a run of the
/// stage used when the dialog opened. Choosing another closes the box the
/// last choice opened while its text is unchanged, so that a file the arrow
/// keys pass opens no box.
#[test_log::test(tokio::test)]
async fn choosing_a_file_of_the_brief_opens_its_box_with_its_text_as_the_dialog_opened() {
    let script = served_script().await;
    let asked = r#"
const steps = [];
serve({ "prompts/design.md": "design as opened", "prompts/fix-ci.md": "fix-ci as opened" });
open();
serve({ "prompts/design.md": "design as serve read it later" });
choose("prompts/design.md");
steps.push(boxes());
choose("prompts/fix-ci.md");
steps.push(boxes());
files.boxes[0].querySelector("textarea").value = "mine";
choose("prompts/design.md");
steps.push(boxes());
choose("prompts/fix-ci.md");
steps.push(boxes());
steps.join("\n")
"#;

    let steps: Vec<String> = evaluated(&[BROWSER, FILTERS, NEW_BENCHMARK_RUN, &script, asked])
        .lines()
        .map(str::to_string)
        .collect();

    assert_eq!(
        steps,
        [
            "prompts/design.md c1.briefs/prompts/design.md: design as opened (started design as opened)",
            "prompts/fix-ci.md c1.briefs/prompts/fix-ci.md: fix-ci as opened (started fix-ci as opened)",
            "prompts/fix-ci.md c1.briefs/prompts/fix-ci.md: mine (started fix-ci as opened) | \
             prompts/design.md c1.briefs/prompts/design.md: design as opened (started design as opened)",
            "prompts/fix-ci.md c1.briefs/prompts/fix-ci.md: mine (started fix-ci as opened)",
        ]
    );
}

/// `docs/specs/web/shell.md`, Expanding and collapsing, Remembered: the foot
/// button collapses the sidebar to the rail and expands it again at once,
/// naming what it does next, and posts each choice to the URL it holds for
/// the server to remember, once the one before is answered, without
/// writing a cookie itself.
#[test_log::test(tokio::test)]
async fn the_foot_button_collapses_and_expands_the_navigation_and_posts_each_choice_in_turn() {
    let script = served_script().await;

    let asked = r#"
var clicks = [];
document.addEventListener = (type, listener) => { if (type === "click") clicks.push(listener); };
var cookies = [];
Object.defineProperty(document, "cookie", { set(cookie) { cookies.push(cookie); } });
var posts = [];
var answers = [];
function URLSearchParams(fields) { this.fields = fields; }
URLSearchParams.prototype.toString = function () {
  return Object.entries(this.fields).map(([name, value]) => `${name}=${value}`).join("&");
};
var fetch = (url, options) => {
  posts.push(`${options.method} ${url} ${options.body}${options.keepalive ? " keepalive" : ""}`);
  return new Promise((answer) => answers.push(answer));
};
var drawer = {
  attributes: {},
  getAttribute(name) { return name in this.attributes ? this.attributes[name] : null; },
  setAttribute(name, value) { this.attributes[name] = value; },
  removeAttribute(name) { delete this.attributes[name]; },
};
var name = { textContent: "Collapse navigation" };
var button = {
  closest: (selector) =>
    selector === "button[data-navigation-toggle]" ? button : selector === ".drawer" ? drawer : null,
  getAttribute: (attribute) =>
    ({ "data-navigation-toggle": "/navigation", "aria-labelledby": "navigation-toggle-name" })[attribute] ?? null,
};
document.getElementById = (id) => (id === "navigation-toggle-name" ? name : null);
var shown = [];
var click = () => {
  for (const listener of clicks) listener({ target: button });
  shown.push(`${drawer.getAttribute("data-navigation")}: ${name.textContent}`);
};
"#;
    let steps = [
        "click(); click();",
        "shown.push(`posted: ${posts.join(\", \")}`); answers[0]({ status: 204 });",
        "shown.push(`posted: ${posts.join(\", \")}`); answers[1]({ status: 204 });",
    ];
    let read = "shown.push(`cookies written: ${cookies.length}`); shown.join(\"\\n\")";
    let seen = settled(&[BROWSER, asked, &script, steps[0], steps[1], steps[2], read]);

    assert_eq!(
        seen.lines().collect::<Vec<_>>(),
        [
            "rail: Expand navigation",
            "null: Collapse navigation",
            "posted: POST /navigation navigation=rail keepalive",
            "posted: POST /navigation navigation=rail keepalive, POST /navigation navigation=sidebar keepalive",
            "cookies written: 0",
        ]
    );
}
