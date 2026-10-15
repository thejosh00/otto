//! `artifacts/todo.md`: a run's checklist, when its work comes in pieces — twenty repos to bump,
//! four things to build, a cleanup that must not be forgotten. A wake writes it and keeps it true
//! (`harness/wake.md`, "The todo"); otto only reads it, to show how far along a run is (`otto
//! show`, the web page) and to say so when a run is marked `done` with items still open.
//!
//! It lives in `artifacts/` because it is the wake's, like every other artifact. One writer means a
//! person on the page can never race a wake halfway through an edit; a person steers the list with
//! a note, and the wake changes it.
//!
//! The format is a markdown task list, so it reads the same in a terminal, an editor and the page:
//!
//! - `- [ ]` to do, `- [~]` doing, `- [x]` done, `- [-]` skipped and `- [!]` failed — the last two
//!   with why, in the item's text.
//! - An item is a line at the left margin. Lines indented under it are its detail, not items.
//! - `#` headings group items. Any other line is prose: kept for the page, never counted.
//!
//! Why five marks rather than GitHub's two: a skipped or failed item is *accounted for*. The gap
//! DESIGN.md §19.1 found was a phase that silently never happened. An item marked skipped, with a
//! reason, is a decision a person can read; an item still `[ ]` when the run says `done` is the
//! omission, and that is the one thing otto flags.

use serde::Serialize;
use std::path::Path;

/// Relative to the run directory.
pub const TODO_FILE: &str = "artifacts/todo.md";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Mark {
    Open,
    Doing,
    Done,
    Skipped,
    Failed,
}

impl Mark {
    fn from_box(c: char) -> Option<Mark> {
        Some(match c {
            ' ' => Mark::Open,
            '~' => Mark::Doing,
            'x' | 'X' => Mark::Done,
            '-' => Mark::Skipped,
            '!' => Mark::Failed,
            _ => return None,
        })
    }

    /// Not yet accounted for: still to do, or under way.
    pub fn is_open(self) -> bool {
        matches!(self, Mark::Open | Mark::Doing)
    }
}

/// One line of the file, in order, as the page shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Line {
    Heading { level: usize, text: String },
    Item { mark: Mark, text: String, detail: Vec<String> },
    Text { text: String },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Counts {
    pub total: usize,
    pub open: usize,
    pub doing: usize,
    pub done: usize,
    pub skipped: usize,
    pub failed: usize,
}

impl Counts {
    /// Items not yet accounted for.
    pub fn left(&self) -> usize {
        self.open + self.doing
    }

    /// `12 done · 1 doing · 3 to do · 2 skipped · 1 failed`, leaving out what is zero.
    pub fn summary(&self) -> String {
        let parts = [(self.done, "done"), (self.doing, "doing"), (self.open, "to do"), (self.skipped, "skipped"), (self.failed, "failed")];
        parts.iter().filter(|(n, _)| *n > 0).map(|(n, what)| format!("{n} {what}")).collect::<Vec<_>>().join(" · ")
    }

    fn add(&mut self, mark: Mark) {
        self.total += 1;
        match mark {
            Mark::Open => self.open += 1,
            Mark::Doing => self.doing += 1,
            Mark::Done => self.done += 1,
            Mark::Skipped => self.skipped += 1,
            Mark::Failed => self.failed += 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Todo {
    pub lines: Vec<Line>,
    pub counts: Counts,
}

impl Todo {
    pub fn parse(text: &str) -> Todo {
        let mut lines: Vec<Line> = Vec::new();
        let mut counts = Counts::default();
        for raw in text.lines() {
            if raw.trim().is_empty() {
                continue;
            }
            if raw.starts_with("  ") || raw.starts_with('\t') {
                if let Some(Line::Item { detail, .. }) = lines.last_mut() {
                    detail.push(raw.trim().to_string());
                    continue;
                }
            }
            let line = raw.trim();
            lines.push(if let Some((mark, text)) = item(line) {
                counts.add(mark);
                Line::Item { mark, text: text.to_string(), detail: Vec::new() }
            } else if let Some((level, text)) = heading(line) {
                Line::Heading { level, text: text.to_string() }
            } else {
                Line::Text { text: line.to_string() }
            });
        }
        Todo { lines, counts }
    }

    /// The text of every item not yet accounted for, in order.
    pub fn open_items(&self) -> Vec<String> {
        self.lines
            .iter()
            .filter_map(|line| match line {
                Line::Item { mark, text, .. } if mark.is_open() => Some(text.clone()),
                _ => None,
            })
            .collect()
    }
}

/// `- [x] text`, `* [ ] text`, `+ [!]`: the mark and the text after it.
fn item(line: &str) -> Option<(Mark, &str)> {
    let rest = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")).or_else(|| line.strip_prefix("+ "))?;
    let mut chars = rest.chars();
    let (open, mark, close) = (chars.next()?, chars.next()?, chars.next()?);
    let text = chars.as_str();
    if open != '[' || close != ']' || !(text.is_empty() || text.starts_with(' ')) {
        return None;
    }
    Some((Mark::from_box(mark)?, text.trim()))
}

/// `## Upgrade`: its level and text. `#123 merged` is not a heading.
fn heading(line: &str) -> Option<(usize, &str)> {
    let rest = line.trim_start_matches('#');
    let level = line.len() - rest.len();
    (level > 0 && (rest.is_empty() || rest.starts_with(' '))).then(|| (level, rest.trim()))
}

/// The run's todo, or `None` when it keeps none: no file, or a file with no items in it, which has
/// no progress to show.
pub fn read(run_dir: &Path) -> Option<Todo> {
    let todo = Todo::parse(&std::fs::read_to_string(run_dir.join(TODO_FILE)).ok()?);
    (todo.counts.total > 0).then_some(todo)
}

#[cfg(test)]
mod tests {
    use super::*;

    const UPGRADE: &str = "# requests → 2.32.3\n\
        \n\
        Bumped one repo at a time.\n\
        \n\
        ## Upgrade\n\
        - [x] acme-web — green at a1b2c3d\n\
        - [!] acme-api — red: 3 failures\n\
        \x20 see artifacts/targets/acme-api/\n\
        - [-] acme-cli — skipped: already at 2.31.0\n\
        - [~] acme-jobs\n\
        \n\
        ## Report\n\
        - [ ] every lock released\n\
        * [X] summary written\n";

    #[test]
    fn each_mark_is_counted_and_the_rest_is_kept_in_order() {
        let todo = Todo::parse(UPGRADE);
        assert_eq!(
            todo.counts,
            Counts { total: 6, open: 1, doing: 1, done: 2, skipped: 1, failed: 1 }
        );
        assert_eq!(todo.counts.left(), 2);
        assert_eq!(todo.counts.summary(), "2 done · 1 doing · 1 to do · 1 skipped · 1 failed");
        assert_eq!(todo.lines[0], Line::Heading { level: 1, text: "requests → 2.32.3".into() });
        assert_eq!(todo.lines[1], Line::Text { text: "Bumped one repo at a time.".into() });
        assert_eq!(todo.lines[2], Line::Heading { level: 2, text: "Upgrade".into() });
        assert_eq!(
            todo.lines[4],
            Line::Item {
                mark: Mark::Failed,
                text: "acme-api — red: 3 failures".into(),
                detail: vec!["see artifacts/targets/acme-api/".into()],
            }
        );
    }

    #[test]
    fn open_items_are_the_ones_not_yet_accounted_for() {
        assert_eq!(Todo::parse(UPGRADE).open_items(), vec!["acme-jobs", "every lock released"]);
    }

    #[test]
    fn what_is_not_a_task_is_not_counted() {
        let todo = Todo::parse(
            "- plain bullet\n- [x]no space after the box\n- [?] unknown mark\n#123 merged\n  - [ ] indented, with no item above\n- []\n",
        );
        assert_eq!(todo.counts.total, 1, "only the indented one, read as an item since nothing is above it");
        assert_eq!(todo.lines[3], Line::Text { text: "#123 merged".into() });
    }

    #[test]
    fn an_item_s_detail_is_not_itself_an_item() {
        let todo = Todo::parse("- [x] parent\n  - [ ] nested\n\n  still the parent's\n");
        assert_eq!(todo.counts.total, 1);
        let Line::Item { detail, .. } = &todo.lines[0] else { panic!("an item") };
        assert_eq!(detail, &["- [ ] nested", "still the parent's"]);
    }

    #[test]
    fn a_run_keeps_a_todo_only_when_its_file_has_items() {
        let dir = std::env::temp_dir().join(format!("otto-todo-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("artifacts")).unwrap();
        assert!(read(&dir).is_none(), "no file");
        std::fs::write(dir.join(TODO_FILE), "# Nothing to tick off yet\n").unwrap();
        assert!(read(&dir).is_none(), "no items");
        std::fs::write(dir.join(TODO_FILE), "- [ ] one\n").unwrap();
        assert_eq!(read(&dir).unwrap().counts.total, 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
