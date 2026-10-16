use std::path::Path;

use anyhow::{Context, Result};

#[derive(Clone, Debug)]
pub struct Briefs {
    pub workflow: String,
    pub plugin: Vec<(&'static str, String)>,
}

const PLUGIN: [(&str, &str); 11] = [
    (
        ".claude-plugin/plugin.json",
        include_str!("../briefs/.claude-plugin/plugin.json"),
    ),
    ("LICENSE", include_str!("../briefs/LICENSE")),
    (
        "skills/triage/SKILL.md",
        include_str!("../briefs/skills/triage/SKILL.md"),
    ),
    (
        "skills/to-tickets/SKILL.md",
        include_str!("../briefs/skills/to-tickets/SKILL.md"),
    ),
    (
        "skills/implement/SKILL.md",
        include_str!("../briefs/skills/implement/SKILL.md"),
    ),
    (
        "skills/tdd/SKILL.md",
        include_str!("../briefs/skills/tdd/SKILL.md"),
    ),
    (
        "skills/tdd/tests.md",
        include_str!("../briefs/skills/tdd/tests.md"),
    ),
    (
        "skills/tdd/mocking.md",
        include_str!("../briefs/skills/tdd/mocking.md"),
    ),
    (
        "skills/code-review/SKILL.md",
        include_str!("../briefs/skills/code-review/SKILL.md"),
    ),
    (
        "skills/pr/SKILL.md",
        include_str!("../briefs/skills/pr/SKILL.md"),
    ),
    (
        "skills/pr/CREDITS.md",
        include_str!("../briefs/skills/pr/CREDITS.md"),
    ),
];

impl Briefs {
    pub fn shipped() -> Briefs {
        Briefs {
            workflow: include_str!("../briefs/workflow.md").into(),
            plugin: PLUGIN
                .iter()
                .map(|(path, text)| (*path, text.to_string()))
                .collect(),
        }
    }

    pub fn load(dir: &Path) -> Result<Briefs> {
        let read = |path: &Path| {
            std::fs::read_to_string(path)
                .with_context(|| format!("reading the brief {}", path.display()))
        };
        let workflow = read(&dir.join("workflow.md"));
        let plugin = PLUGIN
            .iter()
            .map(|(path, _)| Ok((*path, read(&dir.join(path))?)))
            .collect::<Result<_>>()?;
        Ok(Briefs {
            workflow: workflow?,
            plugin,
        })
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
