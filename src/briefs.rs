use std::path::Path;

use anyhow::{Context, Result};

use crate::prompts::{Prompts, Source};

#[derive(Clone, Debug)]
pub struct Briefs {
    /// The Claude Code plugin given to every session, from `plugin/`.
    pub plugin: Vec<(&'static str, String)>,
    /// What Linewright sends sessions, from `prompts/`.
    pub prompts: Prompts,
}

const PLUGIN: [(&str, &str); 12] = [
    (
        ".claude-plugin/plugin.json",
        include_str!("../briefs/plugin/.claude-plugin/plugin.json"),
    ),
    ("LICENSE", include_str!("../briefs/plugin/LICENSE")),
    (
        "skills/triage/SKILL.md",
        include_str!("../briefs/plugin/skills/triage/SKILL.md"),
    ),
    (
        "skills/to-tickets/SKILL.md",
        include_str!("../briefs/plugin/skills/to-tickets/SKILL.md"),
    ),
    (
        "skills/software-design/SKILL.md",
        include_str!("../briefs/plugin/skills/software-design/SKILL.md"),
    ),
    (
        "skills/implement/SKILL.md",
        include_str!("../briefs/plugin/skills/implement/SKILL.md"),
    ),
    (
        "skills/tdd/SKILL.md",
        include_str!("../briefs/plugin/skills/tdd/SKILL.md"),
    ),
    (
        "skills/tdd/tests.md",
        include_str!("../briefs/plugin/skills/tdd/tests.md"),
    ),
    (
        "skills/tdd/mocking.md",
        include_str!("../briefs/plugin/skills/tdd/mocking.md"),
    ),
    (
        "skills/code-review/SKILL.md",
        include_str!("../briefs/plugin/skills/code-review/SKILL.md"),
    ),
    (
        "skills/pr/SKILL.md",
        include_str!("../briefs/plugin/skills/pr/SKILL.md"),
    ),
    (
        "skills/pr/CREDITS.md",
        include_str!("../briefs/plugin/skills/pr/CREDITS.md"),
    ),
];

impl Briefs {
    pub fn shipped() -> Briefs {
        Briefs {
            plugin: PLUGIN
                .iter()
                .map(|(path, text)| (*path, text.to_string()))
                .collect(),
            prompts: Prompts::read(Source::Shipped)
                .expect("the shipped prompts have their values' placeholders"),
        }
    }

    /// Reads the briefs from `dir`, laid out as `briefs/` is.
    pub fn load(dir: &Path) -> Result<Briefs> {
        let read = |path: &Path| {
            std::fs::read_to_string(path)
                .with_context(|| format!("reading the brief {}", path.display()))
        };
        let plugin = PLUGIN
            .iter()
            .map(|(path, _)| Ok((*path, read(&dir.join("plugin").join(path))?)))
            .collect::<Result<_>>()?;
        let prompts = Prompts::read(Source::Dir(&dir.join("prompts")))?;
        Ok(Briefs { plugin, prompts })
    }

    pub fn write_plugin(&self, dir: &Path) -> std::io::Result<()> {
        for (path, text) in &self.plugin {
            let path = dir.join(path);
            std::fs::create_dir_all(path.parent().expect("plugin files sit in a directory"))?;
            std::fs::write(path, text)?;
        }
        Ok(())
    }
}
