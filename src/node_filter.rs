use adw::glib;
use std::{fs, path::PathBuf};

#[derive(Default)]
pub struct NodeFilter {
    patterns: Vec<String>,
    path: PathBuf,
}
impl NodeFilter {
    pub fn load() -> Self {
        let path = glib::user_config_dir().join("conduit/hidden-node-patterns");
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
        if let Some(parent) = self.path.parent() {
            if let Err(error) = fs::create_dir_all(parent)
                .and_then(|_| fs::write(&self.path, self.patterns.join("\n")))
            {
                log::warn!("Could not save hidden node patterns: {error}");
            }
        }
    }
}
