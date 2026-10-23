//! The page's script, run against stand-ins for the browser and for htmx:
//! the hooks it gives htmx, as htmx 4 calls them.

use boa_engine::{Context, Source};

use super::*;

/// Stand-ins for what the script reads of the browser at load, with
/// `htmx.registerExtension` keeping each extension by name in `extensions`.
const BROWSER: &str = r#"
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
var location = { hash: "" };
function setInterval() {}
function setTimeout() {}
"#;

/// The script the board serves, as its page names it.
async fn served_script(web: &Web) -> String {
    let html = web.page("/").await;
    let src = select(&html, "script[src]")
        .into_iter()
        .filter_map(|script| script.attr("src"))
        .find(|src| src.starts_with("/assets/linewright-"))
        .expect("the page's script")
        .to_string();
    let (status, script) = web.get(&src).await;
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

#[tokio::test]
async fn an_update_neither_opens_nor_closes_a_fold_and_morphs_every_other_attribute() {
    let web = Web::start(with_ticket().await).await;
    let script = served_script(&web).await;
    web.stop().await;

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

#[tokio::test]
async fn escape_hides_a_tooltip_shown_until_the_pointer_leaves_it_or_focus_moves() {
    let web = Web::start(with_ticket().await).await;
    let script = served_script(&web).await;
    web.stop().await;

    // Two tooltips, the first under the pointer, with the listeners the
    // script adds to them and to the document.
    let tooltips = r#"
var keydown = null;
function tooltip(shown) {
  return {
    shown,
    dataset: { tip: "owner/widgets" },
    listeners: {},
    addEventListener(type, listener) { this.listeners[type] = listener; },
    matches(selector) { return this.shown && selector.includes(":hover"); },
  };
}
var hovered = tooltip(true);
var other = tooltip(false);
document.querySelectorAll = (selector) => selector === "[data-tip]" ? [hovered, other] : [];
document.addEventListener = (type, listener) => { if (type === "keydown") keydown = listener; };
"#;
    let asked = r#"
const tips = () => `${hovered.dataset.tip}|${other.dataset.tip}`;
const steps = [];
keydown({ key: "Enter" });
steps.push(tips());
keydown({ key: "Escape" });
steps.push(tips());
hovered.listeners.pointerleave({});
steps.push(tips());
keydown({ key: "Escape" });
steps.push(tips());
hovered.listeners.focusout({});
steps.push(tips());
steps.join("\n")
"#;
    let answers = evaluated(&[BROWSER, tooltips, &script, asked]);

    // An empty `data-tip` is a daisyUI tooltip with nothing to show.
    assert_eq!(
        answers.lines().collect::<Vec<_>>(),
        [
            "owner/widgets|owner/widgets",
            "|owner/widgets",
            "owner/widgets|owner/widgets",
            "|owner/widgets",
            "owner/widgets|owner/widgets",
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
  const shown = { title: "", textContent: "", dataset: {}, ...attributes };
  shown.removeAttribute = (name) => { if (name === "title") shown.title = ""; };
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
    let web = Web::start(with_ticket().await).await;
    let script = served_script(&web).await;
    web.stop().await;
    let clock = format!("var now = Date.parse({NOW:?});");
    evaluated(&[BROWSER, &clock, TIMES, page, &script, asked])
        .lines()
        .map(str::to_string)
        .collect()
}

/// A `<time>` for each of `datetimes`.
fn times(datetimes: &[&str]) -> String {
    datetimes
        .iter()
        .map(|at| format!("time({{ dateTime: {at:?} }});\n"))
        .collect()
}

#[tokio::test]
async fn a_past_time_reads_in_words_rounded_down_then_as_its_date_past_30_days() {
    // Each as old, as of `NOW`, as its comment says.
    let datetimes = [
        "2026-10-23T11:59:01Z", // 59 s
        "2026-10-23T11:59:00Z", // 1 min
        "2026-10-23T11:00:01Z", // 59 min 59 s
        "2026-10-23T11:00:00Z", // 1 h
        "2026-10-22T12:01:00Z", // 23 h 59 min
        "2026-10-22T12:00:00Z", // 1 day
        "2026-10-21T13:00:00Z", // 47 h, two calendar days back
        "2026-09-23T12:00:00Z", // 30 days
        "2026-09-23T11:59:59Z", // 30 days and 1 s
        "2025-09-12T10:00:00Z", // a past year
    ];

    let shown = on_the_page(
        &times(&datetimes),
        r#"times.map((time) => `${time.textContent} | ${time.title}`).join("\n")"#,
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
    let absolute = r#"date(en, {"year":"numeric","month":"short","day":"numeric","hour":"2-digit","minute":"2-digit","second":"2-digit","timeZoneName":"short"})"#;
    let titles: Vec<&str> = shown
        .iter()
        .map(|line| line.split(" | ").nth(1).unwrap())
        .collect();
    assert_eq!(
        titles,
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
        .map(|at| format!("{absolute}: {at}")),
        "the absolute time on hover"
    );
}

#[tokio::test]
async fn a_runs_duration_reads_its_two_largest_units_that_are_not_zero() {
    // 45 s, 1 min 43 s, 3 h 2 min 7 s, 1 day 0 h 5 min 9 s and none, settled,
    // then 3 h 2 min 7 s of a run in flight.
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
