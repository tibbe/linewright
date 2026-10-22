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
