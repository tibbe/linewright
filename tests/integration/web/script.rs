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
    let mut context = Context::default();
    for source in [BROWSER, &script] {
        context.eval(Source::from_bytes(source)).unwrap();
    }
    let answers = context.eval(Source::from_bytes(asked)).unwrap();
    let answers = answers
        .to_string(&mut context)
        .unwrap()
        .to_std_string_escaped();

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
