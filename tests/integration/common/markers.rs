use std::sync::Arc;

use linewright::briefs::Briefs;
use linewright::prompts::OwnTemplates;

/// Linewright's own layer with each file a marker: its path and each of its
/// placeholders, as `marker` writes them, on one line.
#[track_caller]
pub fn markers() -> OwnTemplates {
    OwnTemplates::generate(|path, names| {
        let placeholders: Vec<(&str, String)> = names
            .iter()
            .map(|name| (*name, format!("{{{{{name}}}}}")))
            .collect();
        let values: Vec<(&str, &str)> = placeholders
            .iter()
            .map(|(name, placeholder)| (*name, placeholder.as_str()))
            .collect();
        format!("{}\n", marker(path, &values))
    })
    .expect("every marker has its file's placeholders")
}

/// The briefs with no layer over the markers.
#[track_caller]
pub fn marker_briefs() -> Arc<Briefs> {
    Arc::new(Briefs::resolve(&markers(), &[]).briefs)
}

/// The marker of the file at `path` under `briefs/` filled with `values`,
/// each by its name, in the code's order: `[path name=«value» …]`, with no
/// newline after it.
pub fn marker(path: &str, values: &[(&str, &str)]) -> String {
    let mut marker = format!("[{path}");
    for (name, value) in values {
        marker.push_str(&format!(" {name}=«{value}»"));
    }
    marker.push(']');
    marker
}
