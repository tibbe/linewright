use std::collections::BTreeMap;

use linewright::briefs::{BriefLayer, Briefs, Unusable};
use linewright::layer::Layer;
use linewright::prompts;

fn layer(layer: Layer, files: &[(&str, &str)]) -> BriefLayer {
    BriefLayer {
        layer,
        files: files
            .iter()
            .map(|(path, text)| (path.to_string(), text.to_string()))
            .collect(),
    }
}

fn plugin_file<'a>(briefs: &'a Briefs, path: &str) -> &'a str {
    let (_, text) = briefs.plugin.iter().find(|(p, _)| *p == path).unwrap();
    text
}

#[test]
fn each_file_comes_from_the_first_layer_that_has_it_which_is_recorded() {
    let repository = layer(
        Layer::Repository,
        &[
            (
                "prompts/follow-ups/parse-error.md",
                "The repository's: {{error}}",
            ),
            ("plugin/skills/tdd/SKILL.md", "The repository's TDD."),
        ],
    );
    let operator = layer(
        Layer::Operator,
        &[
            (
                "prompts/follow-ups/parse-error.md",
                "The operator's: {{error}}",
            ),
            ("plugin/skills/tdd/SKILL.md", "The operator's TDD."),
            ("plugin/skills/pr/SKILL.md", "The operator's PR."),
        ],
    );

    let resolved = Briefs::resolve(&[&repository, &operator]);

    assert_eq!(resolved.unusable, []);
    let briefs = resolved.briefs;
    assert_eq!(
        prompts::parse_error(&briefs.prompts, "no JSON"),
        "The repository's: no JSON"
    );
    assert_eq!(
        plugin_file(&briefs, "skills/tdd/SKILL.md"),
        "The repository's TDD."
    );
    assert_eq!(
        plugin_file(&briefs, "skills/pr/SKILL.md"),
        "The operator's PR."
    );
    assert_eq!(
        plugin_file(&briefs, "LICENSE"),
        include_str!("../../briefs/plugin/LICENSE")
    );
    let layers: BTreeMap<&str, Layer> = briefs
        .layers()
        .iter()
        .map(|(path, layer)| (path.as_str(), *layer))
        .collect();
    assert_eq!(
        layers["prompts/follow-ups/parse-error.md"],
        Layer::Repository
    );
    assert_eq!(layers["plugin/skills/tdd/SKILL.md"], Layer::Repository);
    assert_eq!(layers["plugin/skills/pr/SKILL.md"], Layer::Operator);
    assert_eq!(layers["plugin/LICENSE"], Layer::Linewright);
    assert_eq!(layers["prompts/triage.md"], Layer::Linewright);
}

#[test]
fn a_prompt_that_cannot_be_used_is_reported_and_the_next_layers_stands_in() {
    let repository = layer(
        Layer::Repository,
        &[("prompts/follow-ups/parse-error.md", "No placeholder.")],
    );
    let operator = layer(
        Layer::Operator,
        &[(
            "prompts/follow-ups/parse-error.md",
            "The operator's: {{error}}",
        )],
    );

    let resolved = Briefs::resolve(&[&repository, &operator]);

    assert_eq!(
        resolved.unusable,
        [Unusable {
            path: "prompts/follow-ups/parse-error.md".into(),
            layer: Layer::Repository,
            error: "it lacks the placeholders `{{error}}`".into(),
        }]
    );
    assert_eq!(
        prompts::parse_error(&resolved.briefs.prompts, "no JSON"),
        "The operator's: no JSON"
    );
    assert_eq!(
        resolved.briefs.layers()["prompts/follow-ups/parse-error.md"],
        Layer::Operator
    );
}
