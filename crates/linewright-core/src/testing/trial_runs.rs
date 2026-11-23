use std::path::Path;

use crate::testing::write_files;
use crate::trial::Configuration;

/// The configuration in `dir`, holding `files` by their paths in it.
#[track_caller]
pub fn configuration(dir: &Path, files: &[(&str, &str)]) -> Configuration {
    std::fs::create_dir_all(dir).unwrap();
    write_files(dir, files);
    Configuration::read(dir).unwrap_or_else(|e| panic!("{e}"))
}

/// The names of the entries in `dir`.
#[track_caller]
pub fn entries(dir: &Path) -> Vec<String> {
    let mut names = vec![];
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("listing {}: {e}", dir.display()))
    {
        let entry = entry.unwrap_or_else(|e| panic!("listing {}: {e}", dir.display()));
        names.push(entry.file_name().to_string_lossy().into_owned());
    }
    names.sort();
    names
}
