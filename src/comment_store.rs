// Copyright 2026 spacedouut
//
// SPDX-License-Identifier: GPL-3.0-only

use std::{fs, path::PathBuf};

use crate::persistence::{self, decode as decode_key, encode as encode_key};

/// A text note placed on the graph.
pub struct Comment {
    pub id: u32,
    pub x: f32,
    pub y: f32,
    pub text: String,
}

#[derive(Default)]
pub struct CommentStore {
    comments: Vec<Comment>,
    path: PathBuf,
    /// Text was edited since the last save.
    dirty: bool,
}

impl CommentStore {
    pub fn load() -> Self {
        Self::load_from(persistence::config_path("comments"))
    }

    fn load_from(path: PathBuf) -> Self {
        let comments = fs::read_to_string(&path)
            .map(|contents| contents.lines().filter_map(parse_comment).collect())
            .unwrap_or_default();
        Self {
            comments,
            path,
            dirty: false,
        }
    }

    pub fn comments(&self) -> &[Comment] {
        &self.comments
    }

    pub fn add(&mut self, x: f32, y: f32) -> u32 {
        let id = self
            .comments
            .iter()
            .map(|comment| comment.id + 1)
            .max()
            .unwrap_or(0);
        self.comments.push(Comment {
            id,
            x,
            y,
            text: String::new(),
        });
        self.save();
        id
    }

    /// Change a comment's text. The file is not written until [`Self::flush`],
    /// so typing does not rewrite it for every keystroke.
    pub fn set_text(&mut self, id: u32, text: &str) {
        if let Some(comment) = self.comments.iter_mut().find(|comment| comment.id == id) {
            if comment.text != text {
                comment.text = text.to_owned();
                self.dirty = true;
            }
        }
    }

    pub fn needs_save(&self) -> bool {
        self.dirty
    }

    /// Write text edits that have not been saved yet.
    pub fn flush(&mut self) {
        if self.dirty {
            self.save();
        }
    }

    pub fn set_position(&mut self, id: u32, x: f32, y: f32) {
        if let Some(comment) = self.comments.iter_mut().find(|comment| comment.id == id) {
            comment.x = x;
            comment.y = y;
            self.save();
        }
    }

    pub fn remove(&mut self, id: u32) {
        self.comments.retain(|comment| comment.id != id);
        self.save();
    }

    fn save(&mut self) {
        self.dirty = false;
        persistence::write_atomically(&self.path, &serialize(&self.comments), "comments");
    }
}

fn serialize(comments: &[Comment]) -> String {
    comments
        .iter()
        .map(|comment| {
            format!(
                "{}\t{}\t{}\t{}\n",
                comment.id,
                comment.x,
                comment.y,
                encode_key(&comment.text)
            )
        })
        .collect()
}

fn parse_comment(line: &str) -> Option<Comment> {
    let mut fields = line.split('\t');
    Some(Comment {
        id: fields.next()?.parse().ok()?,
        x: fields.next()?.parse().ok()?,
        y: fields.next()?.parse().ok()?,
        text: decode_key(fields.next().unwrap_or(""))?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved(store: &CommentStore) -> String {
        fs::read_to_string(&store.path).unwrap_or_default()
    }

    #[test]
    fn parses_saved_comment() {
        let comment = parse_comment("3\t-12.5\t40\t68690a7468657265").unwrap();
        assert_eq!(comment.id, 3);
        assert_eq!((comment.x, comment.y), (-12.5, 40.0));
        assert_eq!(comment.text, "hi\nthere");
    }

    #[test]
    fn serializes_in_the_saved_format() {
        let comments = [
            Comment {
                id: 3,
                x: -12.5,
                y: 40.0,
                text: "hi\nthere".into(),
            },
            Comment {
                id: 4,
                x: 0.0,
                y: 1.25,
                text: String::new(),
            },
        ];
        assert_eq!(
            serialize(&comments),
            "3\t-12.5\t40\t68690a7468657265\n4\t0\t1.25\t\n"
        );
    }

    #[test]
    fn typing_does_not_write_until_flushed() {
        let directory = persistence::test_directory("comment-typing");
        let mut store = CommentStore::load_from(directory.join("comments"));
        let id = store.add(1.0, 2.0);
        let before = saved(&store);
        assert_eq!(before, "0\t1\t2\t\n");

        for text in ["h", "he", "hey"] {
            store.set_text(id, text);
            assert_eq!(saved(&store), before);
        }
        assert!(store.needs_save());

        store.flush();
        assert!(!store.needs_save());
        assert_eq!(saved(&store), "0\t1\t2\t686579\n");

        let reloaded = CommentStore::load_from(store.path.clone());
        assert_eq!(reloaded.comments()[0].text, "hey");
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn unchanged_text_and_clean_flush_do_not_write() {
        let directory = persistence::test_directory("comment-clean");
        let mut store = CommentStore::load_from(directory.join("comments"));
        let id = store.add(0.0, 0.0);
        store.set_text(id, "");
        assert!(!store.needs_save());
        fs::remove_file(&store.path).unwrap();
        store.flush();
        assert!(!store.path.exists());
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn other_changes_save_pending_text() {
        let directory = persistence::test_directory("comment-pending");
        let mut store = CommentStore::load_from(directory.join("comments"));
        let id = store.add(0.0, 0.0);
        store.set_text(id, "hey");
        store.set_position(id, 5.0, 6.0);
        assert!(!store.needs_save());
        assert_eq!(saved(&store), "0\t5\t6\t686579\n");
        let _ = fs::remove_dir_all(directory);
    }
}
