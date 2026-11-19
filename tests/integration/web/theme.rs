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

/// `docs/specs/web/design_system.md`, Colour scheme: Card is `base-200`,
/// and Muted text is `base-content` at 60% opacity.
#[test_log::test]
fn the_card_and_muted_text_roles_are_base_200_and_base_content_at_60_percent() {
    let roles: Vec<(&str, &str)> = STYLES
        .split("@theme inline {")
        .nth(1)
        .and_then(|rest| rest.split('}').next())
        .expect("an inline theme block")
        .split(';')
        .filter_map(|declaration| declaration.trim().split_once(':'))
        .map(|(variable, value)| (variable.trim(), value.trim()))
        .collect();

    assert_eq!(
        roles,
        [
            ("--color-card", "var(--color-base-200)"),
            (
                "--color-muted",
                "color-mix(in oklab, var(--color-base-content) 60%, transparent)"
            ),
        ]
    );
}
