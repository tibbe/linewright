use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};

use crate::layer::Layer;
use crate::prompts::{self, Prompts, Readers};

/// What a session works from, each file from the first layer that has it.
#[derive(Clone, Debug)]
pub struct Briefs {
    /// The Claude Code plugin given to every session, from `plugin/`.
    pub plugin: Vec<(&'static str, String)>,
    /// What Linewright sends sessions, from `prompts/`.
    pub prompts: Prompts,
    /// The layer of each file, by its path under the briefs' root.
    layers: BTreeMap<String, Layer>,
}

/// One layer of the briefs: each file it has, by its path under the
/// briefs' root, such as `prompts/implement.md`.
#[derive(Clone, Debug)]
pub struct BriefLayer {
    pub layer: Layer,
    pub files: BTreeMap<String, String>,
}

/// A layer's file Linewright cannot use, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unusable {
    pub path: String,
    pub layer: Layer,
    pub error: String,
}

/// The briefs resolved from layers, and the files in them that could not be
/// used, for each of which the next layer's file stands in.
#[derive(Clone, Debug)]
pub struct Resolved {
    pub briefs: Briefs,
    pub unusable: Vec<Unusable>,
}

/// The operator's layer of the briefs, each of whose files can be used,
/// and the briefs it makes over Linewright's.
#[derive(Clone, Debug)]
pub struct OperatorBriefs {
    layer: BriefLayer,
    briefs: Arc<Briefs>,
    /// The files in the `briefs` directory outside Linewright's layout.
    unused: Vec<String>,
}

const PROMPTS_DIR: &str = "prompts/";

/// The path under the briefs' root of the plugin's file at `path`.
fn plugin_path(path: &str) -> String {
    format!("plugin/{path}")
}

/// The path under the briefs' root of the prompt at `path`.
fn prompt_path(path: &str) -> String {
    format!("{PROMPTS_DIR}{path}")
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
        Briefs::resolve(&[]).briefs
    }

    /// Takes each file from the first of `layers` that has it and whose
    /// file can be used, or else from the briefs Linewright ships.
    pub fn resolve(layers: &[&BriefLayer]) -> Resolved {
        let mut layers_of = BTreeMap::new();
        let plugin = PLUGIN
            .iter()
            .map(|(path, shipped)| {
                let key = plugin_path(path);
                let (layer, text) = layers
                    .iter()
                    .find_map(|layer| Some((layer.layer, layer.files.get(&key)?.as_str())))
                    .unwrap_or((Layer::Linewright, shipped));
                layers_of.insert(key, layer);
                (*path, text.to_string())
            })
            .collect();
        // Each path's layers that have it, the one in use first.
        let mut candidates: BTreeMap<&str, Vec<&BriefLayer>> = Prompts::paths()
            .into_iter()
            .map(|path| {
                let key = prompt_path(path);
                let having = layers
                    .iter()
                    .filter(|layer| layer.files.contains_key(&key))
                    .copied()
                    .collect();
                (path, having)
            })
            .collect();
        let mut unusable = Vec::new();
        // A file that cannot be used drops out, and the prompts are read
        // again with the next layer's in its place.
        let prompts = loop {
            let (prompts, failed) = Prompts::read(&mut |path| {
                let layer = candidates.get(path)?.first()?;
                layer.files.get(&prompt_path(path)).map(String::as_str)
            });
            if failed.is_empty() {
                break prompts;
            }
            for (path, error) in failed {
                let having = candidates
                    .get_mut(path)
                    .expect("only a path with a layer's file fails");
                let layer = having.remove(0);
                unusable.push(Unusable {
                    path: prompt_path(path),
                    layer: layer.layer,
                    error: format!("{error:#}"),
                });
            }
        };
        for (path, having) in candidates {
            let layer = having
                .first()
                .map_or(Layer::Linewright, |layer| layer.layer);
            layers_of.insert(prompt_path(path), layer);
        }
        Resolved {
            briefs: Briefs {
                plugin,
                prompts,
                layers: layers_of,
            },
            unusable,
        }
    }

    /// The runs the file at `path` under the briefs' root is sent to, or
    /// `None` for a path outside Linewright's layout. Every run is given the
    /// whole plugin.
    pub fn readers(path: &str) -> Option<Readers> {
        if let Some(prompt) = path.strip_prefix(PROMPTS_DIR) {
            return prompts::readers(prompt);
        }
        PLUGIN
            .iter()
            .any(|(plugin, _)| plugin_path(plugin) == path)
            .then_some(Readers::Every)
    }

    /// The layer each file came from, by its path under the briefs' root.
    pub fn layers(&self) -> &BTreeMap<String, Layer> {
        &self.layers
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

impl OperatorBriefs {
    /// The operator's layer, from the `briefs` directory, laid out as
    /// `briefs/` is, if one is set: the files of Linewright's layout that it
    /// holds, each of which must be usable.
    pub fn read(dir: Option<&Path>) -> Result<OperatorBriefs> {
        let mut layer = BriefLayer {
            layer: Layer::Operator,
            files: BTreeMap::new(),
        };
        let mut unused = Vec::new();
        if let Some(dir) = dir {
            let (used, outside): (Vec<String>, Vec<String>) = files_under(dir)?
                .into_iter()
                .partition(|path| Briefs::readers(path).is_some());
            for path in used {
                let file = dir.join(&path);
                let text = std::fs::read_to_string(&file)
                    .with_context(|| format!("reading the brief {}", file.display()))?;
                layer.files.insert(path, text);
            }
            unused = outside;
        }
        let resolved = Briefs::resolve(&[&layer]);
        if let Some(unusable) = resolved.unusable.first() {
            let prompt = unusable
                .path
                .strip_prefix(PROMPTS_DIR)
                .unwrap_or(&unusable.path);
            return Err(anyhow!("reading the prompt {prompt}: {}", unusable.error));
        }
        Ok(OperatorBriefs {
            layer,
            briefs: Arc::new(resolved.briefs),
            unused,
        })
    }

    pub fn layer(&self) -> &BriefLayer {
        &self.layer
    }

    /// The briefs of a run with no layer above the operator's.
    pub fn briefs(&self) -> Arc<Briefs> {
        self.briefs.clone()
    }

    /// The paths under the `briefs` directory of its files outside
    /// Linewright's layout, which no session is given.
    pub fn unused(&self) -> &[String] {
        &self.unused
    }
}

/// The path under `dir` of every file in it and its subdirectories, in
/// order, with `/` between the parts.
fn files_under(dir: &Path) -> Result<Vec<String>> {
    let mut files = Vec::new();
    let mut dirs = vec![(dir.to_path_buf(), String::new())];
    while let Some((at, prefix)) = dirs.pop() {
        let entries = std::fs::read_dir(&at)
            .with_context(|| format!("reading the briefs directory {}", at.display()))?;
        for entry in entries {
            let entry =
                entry.with_context(|| format!("reading the briefs directory {}", at.display()))?;
            let path = format!("{prefix}{}", entry.file_name().to_string_lossy());
            if entry.path().is_dir() {
                dirs.push((entry.path(), format!("{path}/")));
            } else {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}
