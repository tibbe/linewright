use std::path::PathBuf;

use super::world::World;

const CONFIG: &str = ".linewright/config.toml";

/// The repository's file with `command` as its setup command.
pub fn setup_config(command: &str) -> String {
    format!("[setup]\ncommand = '''{command}'''\n")
}

/// Ignores `node_modules` and sets `command` as the setup command.
pub async fn set_up_with(world: &World, command: &str) {
    let config = setup_config(command);
    let files = [(".gitignore", "node_modules/\n"), (CONFIG, config.as_str())];
    if let Err(e) = world
        .store
        .commit("maintainer", "main", &files, "Change")
        .await
    {
        panic!("setting up failed: {e:#}");
    }
}

/// A directory outside every clone, where setup commands leave marks.
pub struct Marks(tempfile::TempDir);

impl Default for Marks {
    fn default() -> Marks {
        Marks::new()
    }
}

impl Marks {
    pub fn new() -> Marks {
        Marks(tempfile::tempdir().unwrap())
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.0.path().join(name)
    }

    /// A setup command that notes each build, then installs `node_modules/x`
    /// holding `version`.
    pub fn installing(&self, version: &str) -> String {
        format!(
            "echo built >> '{}'; mkdir -p node_modules && echo {version} > node_modules/x",
            self.path("builds").display()
        )
    }

    /// How many builds have run [`Marks::installing`]'s commands, or
    /// written a line to `builds` as they do.
    pub fn builds(&self) -> usize {
        std::fs::read_to_string(self.path("builds"))
            .unwrap_or_default()
            .lines()
            .count()
    }
}
