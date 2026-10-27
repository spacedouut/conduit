// Copyright 2026 spacedouut
//
// SPDX-License-Identifier: GPL-3.0-only

use std::{fs, path::PathBuf};

use adw::glib;

use crate::node_layout::{decode_key, encode_key};

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
}

impl CommentStore {
    pub fn load() -> Self {
        let path = glib::user_config_dir().join("conduit/comments");
        let comments = fs::read_to_string(&path)
            .map(|contents| contents.lines().filter_map(parse_comment).collect())
            .unwrap_or_default();
        Self { comments, path }
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

    pub fn set_text(&mut self, id: u32, text: &str) {
        if let Some(comment) = self.comments.iter_mut().find(|comment| comment.id == id) {
            comment.text = text.to_owned();
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

    fn save(&self) {
        let Some(parent) = self.path.parent() else {
            return;
        };
        if let Err(error) = fs::create_dir_all(parent) {
            log::warn!("Could not create comments directory: {error}");
            return;
        }
        let contents: String = self
            .comments
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
            .collect();
        let temporary_path = self.path.with_extension("tmp");
        if let Err(error) = fs::write(&temporary_path, contents)
            .and_then(|_| fs::rename(&temporary_path, &self.path))
        {
            log::warn!("Could not save comments: {error}");
            let _ = fs::remove_file(temporary_path);
        }
    }
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
    use super::parse_comment;

    #[test]
    fn parses_saved_comment() {
        let comment = parse_comment("3\t-12.5\t40\t68690a7468657265").unwrap();
        assert_eq!(comment.id, 3);
        assert_eq!((comment.x, comment.y), (-12.5, 40.0));
        assert_eq!(comment.text, "hi\nthere");
    }
}
