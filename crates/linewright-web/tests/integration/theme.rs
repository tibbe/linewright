//! The themes (`docs/specs/design_system.md`, Colour), as
//! `styles/linewright.css` sets them, from which `scripts/fix css` builds
//! the stylesheet that the web UI serves.

use linewright_core::testing::repository;

/// The file at `path` from the root of the repository.
///
/// # Panics
///
/// If it can't be read, naming the path.
#[track_caller]
fn repo_file(path: &str) -> String {
    let path = repository().join(path);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()))
}

const STYLES: &str = "styles/linewright.css";

/// Each variable of the spec's table, with its light and its dark value.
#[track_caller]
fn spec_variables() -> Vec<(String, String, String)> {
    repo_file("docs/specs/design_system.md")
        .lines()
        .filter_map(|line| {
            let cells: Vec<&str> = line
                .trim()
                .strip_prefix('|')?
                .strip_suffix('|')?
                .split('|')
                .map(str::trim)
                .collect();
            match cells[..] {
                [variable, light, dark] if variable.starts_with("`--") => {
                    Some((unquoted(variable), unquoted(light), unquoted(dark)))
                }
                _ => None,
            }
        })
        .collect()
}

fn unquoted(cell: &str) -> String {
    cell.trim_matches('`').into()
}

/// The variables that the stylesheet's theme block named `name` sets, in
/// order.
#[track_caller]
fn theme_variables(name: &str) -> Vec<(String, String)> {
    let stylesheet = repo_file(STYLES);
    let blocks: Vec<&str> = stylesheet
        .split("@plugin \"./daisyui-theme.mjs\" {")
        .skip(1)
        .map(|rest| rest.split('}').next().expect("a closed block"))
        .filter(|block| block.contains(&format!("name: \"{name}\";")))
        .collect();
    assert_eq!(blocks.len(), 1, "one theme block named {name}");
    blocks[0]
        .split(';')
        .filter_map(|declaration| {
            let (variable, value) = declaration.trim().split_once(':')?;
            variable
                .starts_with("--")
                .then(|| (variable.trim().into(), value.trim().into()))
        })
        .collect()
}

#[test_log::test]
fn each_theme_sets_exactly_the_variables_of_the_specs_table() {
    let spec = spec_variables();
    assert_eq!(spec.len(), 28, "{spec:?}");

    let light: Vec<(String, String)> = spec
        .iter()
        .map(|(variable, light, _)| (variable.clone(), light.clone()))
        .collect();
    let dark: Vec<(String, String)> = spec
        .iter()
        .map(|(variable, _, dark)| (variable.clone(), dark.clone()))
        .collect();
    assert_eq!(theme_variables("light"), light);
    assert_eq!(theme_variables("dark"), dark);
}

/// `docs/specs/design_system.md`, Colour, The colours drawn: the web UI draws
/// in no colour but daisyUI's, so the stylesheet sets no colour but in its
/// themes.
#[test_log::test]
fn the_stylesheet_sets_no_colour_but_in_its_themes() {
    let stylesheet = repo_file(STYLES);
    let mut parts = stylesheet.split("@plugin \"./daisyui-theme.mjs\" {");
    let outside_the_themes: String = parts
        .next()
        .into_iter()
        .chain(parts.map(|theme| theme.split_once('}').map_or("", |(_, after)| after)))
        .collect();

    assert!(
        !outside_the_themes.contains("--color-"),
        "{outside_the_themes}"
    );
}

/// `docs/specs/design_system.md`, Colour, The colours drawn: a fill is drawn
/// only behind its content colour, so the stylesheet colours no text, outline,
/// border or ring in one.
#[test_log::test]
fn the_stylesheet_colours_nothing_in_a_fill_but_its_background() {
    const FILLS: [&str; 8] = [
        "primary",
        "secondary",
        "accent",
        "neutral",
        "info",
        "success",
        "warning",
        "error",
    ];
    let stylesheet = repo_file(STYLES);
    let coloured: Vec<&str> = stylesheet
        .split_whitespace()
        .map(|class| class.trim_end_matches(';'))
        .filter(|class| {
            let utility = class.rsplit(':').next().unwrap_or(class);
            [
                "text-",
                "outline-",
                "border-",
                "ring-",
                "decoration-",
                "fill-",
                "stroke-",
            ]
            .iter()
            .filter_map(|prefix| utility.strip_prefix(prefix))
            .any(|colour| {
                FILLS
                    .iter()
                    .any(|fill| colour.split('/').next() == Some(*fill))
            })
        })
        .collect();

    assert!(coloured.is_empty(), "{coloured:?}");
}

/// `docs/specs/components.md`, Inline code: `font-mono` at
/// `text-[0.9em]`, on `bg-base-content/10`, with `rounded-sm` corners and a
/// padding of `px-1`.
#[test_log::test]
fn inline_code_is_drawn_as_the_spec_gives_it() {
    let stylesheet = repo_file(STYLES);
    let rule = stylesheet
        .split("  code {")
        .nth(1)
        .and_then(|rest| rest.split('}').next())
        .expect("a rule for code");
    let mut classes: Vec<&str> = rule
        .trim()
        .strip_prefix("@apply")
        .expect("the rule applies classes")
        .trim_end_matches(';')
        .split_whitespace()
        .collect();
    classes.sort_unstable();

    assert_eq!(
        classes,
        [
            "bg-base-content/10",
            "font-mono",
            "px-1",
            "rounded-sm",
            "text-[0.9em]"
        ]
    );
}
