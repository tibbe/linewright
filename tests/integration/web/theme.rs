//! The themes (`docs/specs/web/design_system.md`, Colour scheme), as
//! `styles/linewright.css` sets them, from which `scripts/fix css` builds
//! the stylesheet that the web UI serves.

const SPEC: &str = include_str!("../../../docs/specs/web/design_system.md");
const STYLES: &str = include_str!("../../../styles/linewright.css");

/// Each variable of the spec's table, with its light and its dark value.
fn spec_variables() -> Vec<(String, String, String)> {
    SPEC.lines()
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
    let blocks: Vec<&str> = STYLES
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

/// `docs/specs/web/design_system.md`, Colour scheme: the web UI draws in no
/// colour but daisyUI's, so the stylesheet sets no colour but in its themes.
#[test_log::test]
fn the_stylesheet_sets_no_colour_but_in_its_themes() {
    let mut parts = STYLES.split("@plugin \"./daisyui-theme.mjs\" {");
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

/// `docs/specs/web/components.md`, Inline code: `font-mono` at
/// `text-[0.9em]`, on `bg-base-content/10`, with `rounded-sm` corners and a
/// padding of `px-1`.
#[test_log::test]
fn inline_code_is_drawn_as_the_spec_gives_it() {
    let rule = STYLES
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
