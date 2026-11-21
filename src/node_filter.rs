use std::{fs, path::PathBuf};

use crate::persistence;

#[derive(Default)]
pub struct NodeFilter {
    patterns: Vec<String>,
    path: PathBuf,
}
impl NodeFilter {
    pub fn load() -> Self {
        let path = persistence::config_path("hidden-node-patterns");
        let patterns = fs::read_to_string(&path)
            .map(|text| {
                text.lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        Self { patterns, path }
    }
    pub fn patterns(&self) -> &[String] {
        &self.patterns
    }
    pub fn set_patterns(&mut self, patterns: Vec<String>) {
        self.patterns = patterns;
        persistence::write_atomically(
            &self.path,
            &self.patterns.join("\n"),
            "hidden node patterns",
        );
    }
}
