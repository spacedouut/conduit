use regex::Regex;
use std::{fmt, fs, path::PathBuf};

use crate::persistence;

/// A hidden-node pattern that is not a valid regular expression.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidPattern {
    pub pattern: String,
    pub reason: String,
}

impl fmt::Display for InvalidPattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.pattern, self.reason)
    }
}

/// Hidden-node patterns, compiled once when they are loaded or changed.
pub struct NodeFilter {
    patterns: Vec<String>,
    matchers: Vec<Regex>,
    invalid: Vec<InvalidPattern>,
    path: PathBuf,
}

impl Default for NodeFilter {
    fn default() -> Self {
        Self::from_patterns(Vec::new(), PathBuf::new())
    }
}

impl NodeFilter {
    pub fn load() -> Self {
        let path = persistence::config_path("hidden-node-patterns");
        let patterns = fs::read_to_string(&path)
            .map(|text| parse_patterns(&text))
            .unwrap_or_default();
        let filter = Self::from_patterns(patterns, path);
        for invalid in &filter.invalid {
            log::warn!("Ignoring invalid hidden node pattern {invalid}");
        }
        filter
    }

    fn from_patterns(patterns: Vec<String>, path: PathBuf) -> Self {
        let (matchers, invalid) = compile(&patterns);
        Self {
            patterns,
            matchers,
            invalid,
            path,
        }
    }

    /// Every stored pattern, including invalid ones so they can be corrected.
    pub fn patterns(&self) -> &[String] {
        &self.patterns
    }

    /// Stored patterns that could not be compiled and are therefore not applied.
    pub fn invalid_patterns(&self) -> &[InvalidPattern] {
        &self.invalid
    }

    pub fn is_hidden(&self, node_key: &str) -> bool {
        self.matchers
            .iter()
            .any(|matcher| matcher.is_match(node_key))
    }

    /// Replace and save the patterns. Nothing changes if any pattern is invalid.
    pub fn set_patterns(&mut self, patterns: Vec<String>) -> Result<(), Vec<InvalidPattern>> {
        let (matchers, invalid) = compile(&patterns);
        if !invalid.is_empty() {
            return Err(invalid);
        }
        self.patterns = patterns;
        self.matchers = matchers;
        self.invalid.clear();
        persistence::write_atomically(
            &self.path,
            &self.patterns.join("\n"),
            "hidden node patterns",
        );
        Ok(())
    }
}

pub fn parse_patterns(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

fn compile(patterns: &[String]) -> (Vec<Regex>, Vec<InvalidPattern>) {
    let mut valid = Vec::new();
    let mut invalid = Vec::new();
    for pattern in patterns {
        match Regex::new(pattern) {
            Ok(regex) => valid.push(regex),
            Err(error) => invalid.push(InvalidPattern {
                pattern: pattern.clone(),
                reason: describe(&error),
            }),
        }
    }
    (valid, invalid)
}

/// The regex crate's syntax errors span several lines; keep the summary line.
fn describe(error: &regex::Error) -> String {
    let message = error.to_string();
    let summary = message.lines().last().unwrap_or_default().trim();
    summary
        .strip_prefix("error: ")
        .unwrap_or(summary)
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::{parse_patterns, InvalidPattern, NodeFilter};
    use std::path::PathBuf;

    fn filter(patterns: &[&str], path: PathBuf) -> NodeFilter {
        NodeFilter::from_patterns(patterns.iter().map(|p| p.to_string()).collect(), path)
    }

    fn temporary_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "conduit-node-filter-{}-{name}/hidden-node-patterns",
            std::process::id()
        ))
    }

    #[test]
    fn matches_any_valid_pattern() {
        let filter = filter(&["^alsa_output\\.", "Midi-Bridge"], PathBuf::new());
        assert!(filter.is_hidden("alsa_output.usb-Example-00.analog-stereo"));
        assert!(filter.is_hidden("Midi-Bridge"));
        assert!(!filter.is_hidden("alsa_input.usb-Example-00.mono"));
        assert!(filter.invalid_patterns().is_empty());
    }

    #[test]
    fn empty_filter_hides_nothing() {
        assert!(!NodeFilter::default().is_hidden("anything"));
    }

    #[test]
    fn invalid_patterns_are_reported_and_valid_ones_still_apply() {
        let filter = filter(&["^Firefox$", "(unclosed", "a{2,1}"], PathBuf::new());
        assert!(filter.is_hidden("Firefox"));
        assert!(!filter.is_hidden("(unclosed"));
        assert_eq!(filter.patterns().len(), 3);
        let invalid: Vec<_> = filter
            .invalid_patterns()
            .iter()
            .map(|invalid| invalid.pattern.as_str())
            .collect();
        assert_eq!(invalid, ["(unclosed", "a{2,1}"]);
    }

    #[test]
    fn invalid_pattern_reason_is_a_single_line() {
        let filter = filter(&["(unclosed"], PathBuf::new());
        let InvalidPattern { reason, .. } = &filter.invalid_patterns()[0];
        assert_eq!(reason, "unclosed group");
        assert_eq!(
            filter.invalid_patterns()[0].to_string(),
            "(unclosed: unclosed group"
        );
    }

    #[test]
    fn set_patterns_rejects_invalid_patterns_without_changing_anything() {
        let path = temporary_path("reject");
        let mut filter = filter(&["^keep$"], path.clone());
        let error = filter
            .set_patterns(vec!["^new$".to_owned(), "[z-a]".to_owned()])
            .unwrap_err();
        assert_eq!(error.len(), 1);
        assert_eq!(error[0].pattern, "[z-a]");
        assert_eq!(filter.patterns(), ["^keep$"]);
        assert!(filter.is_hidden("keep"));
        assert!(!filter.is_hidden("new"));
        assert!(!path.exists());
    }

    #[test]
    fn set_patterns_recompiles_and_saves() {
        let path = temporary_path("save");
        let mut filter = filter(&["^old$", "(broken"], path.clone());
        filter
            .set_patterns(vec!["^new$".to_owned(), "other".to_owned()])
            .unwrap();
        assert!(filter.is_hidden("new"));
        assert!(!filter.is_hidden("old"));
        assert!(filter.invalid_patterns().is_empty());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "^new$\nother");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn parse_patterns_trims_and_skips_blank_lines() {
        assert_eq!(parse_patterns("  ^a$ \n\n\t\nb\n"), ["^a$", "b"]);
    }
}
