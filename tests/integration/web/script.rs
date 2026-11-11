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

#[test_log::test(tokio::test)]
async fn an_update_neither_opens_nor_closes_a_fold_and_morphs_every_other_attribute() {
    let script = served_script().await;

    // Each hook as htmx calls it in a morph:
    // https://four.htmx.org/docs/extension-authoring-guide#event-hooks
    let asked = r#"
const hooks = Object.values(extensions)
  .map((extension) => extension.htmx_before_morph_attr)
  .filter(Boolean);
const morphed = (tagName, attrName, newValue) =>
  hooks.every((hook) => hook({ tagName }, { attrName, newValue }) !== false);
[
  ["DETAILS", "open", ""],
  ["DETAILS", "open", null],
  ["DETAILS", "class", "lane"],
  ["DIALOG", "open", ""],
  ["SECTION", "open", null],
].map(([tag, attr, value]) => `${tag} ${attr}=${value}: ${morphed(tag, attr, value)}`)
 .join("\n")
"#;
    let answers = evaluated(&[BROWSER, &script, asked]);

    assert_eq!(
        answers.lines().collect::<Vec<_>>(),
        [
            "DETAILS open=: false",
            "DETAILS open=null: false",
            "DETAILS class=lane: true",
            "DIALOG open=: true",
            "SECTION open=null: true",
        ]
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
/// options)` makes, a field with `options` suggesting them, `fire(type,
/// event)` sends an event to the listeners the script added, `event(target,
/// more)` makes one, and `fold(id, open)` puts a fold on the page in place of
/// any with its id. `applied()` is each address the page took and each stream
/// it restarted since it was last asked, or `-` for none.
const FILTERS: &str = r#"
var listeners = {};
function listen(type, listener) { (listeners[type] = listeners[type] || []).push(listener); }
document.addEventListener = listen;
document.body = { addEventListener: listen, getAttribute() { return null; } };
function fire(type, event) { for (const listener of listeners[type] || []) listener(event); }
function event(target, more) {
  return Object.assign({ target, prevented: false, preventDefault() { this.prevented = true; } }, more);
}
var location = { pathname: "/runs" };
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
  if (options) field.list = { options: options.map((value) => ({ value })) };
  form.elements.push(field);
  return field;
}
var folds = {};
function fold(id, open) {
  folds[id] = Object.assign(element("DETAILS", { id }), { id, open });
  return folds[id];
}
document.getElementById = (id) => (id === "stream" ? stream : folds[id] || null);
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

#[test_log::test(tokio::test)]
async fn a_picked_suggestion_applies_at_once_and_a_partly_typed_one_does_not() {
    let page = r#"var person = field("INPUT", "person", ["alice", "bob"]);"#;
    let asked = r#"
const steps = [];
person.value = "ali"; fire("input", event(person)); steps.push(applied());
person.value = "alice"; fire("input", event(person)); steps.push(applied());
const elsewhere = element("SELECT", {}); fire("change", event(elsewhere)); steps.push(applied());
steps.join("\n")
"#;

    let applied = with_filters(page, asked).await;

    assert_eq!(
        applied,
        [
            "-".to_string(),
            format!(
                "/runs?person=alice | {}",
                restarted("/events/runs?person=alice")
            ),
            "-".to_string(),
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
