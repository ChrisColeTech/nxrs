//! The main window's view state: the Search & Filter popover's fields, the list's column sort, the
//! build output.

use crate::constants::{layout, words_build as w};
use crate::model::filter::LibraryFilter;
use std::collections::VecDeque;

#[derive(Clone, Debug, PartialEq)]
pub struct Filters {
    pub open: bool,
    pub search: String,
    pub state: usize,
    pub source: usize,
    /// 0 Name, 1 Last played, 2 Build status; None = "Custom table order" (a column the user sorted)
    pub sort: Option<usize>,
    pub focus_search: bool,
}
impl Default for Filters {
    fn default() -> Self {
        Filters {
            open: false,
            search: String::new(),
            state: 0,
            source: 0,
            sort: Some(0),
            focus_search: false,
        }
    }
}
impl Filters {
    pub fn query(&self) -> LibraryFilter {
        LibraryFilter {
            text: self.search.clone(),
            state: self.state,
            source: self.source,
        }
    }
    /// Clear filters: one change.
    pub fn clear(&mut self) {
        (self.search, self.state, self.source, self.sort) = (String::new(), 0, 0, Some(0));
        self.focus_search = true;
    }
}

/// The build output: at most 12000 lines, shown or not, its title following the build.
#[derive(Clone, Debug, PartialEq)]
pub struct Console {
    pub lines: VecDeque<String>,
    pub title: String,
}
impl Default for Console {
    fn default() -> Self {
        Console {
            lines: VecDeque::new(),
            title: w::OUTPUT.into(),
        }
    }
}
impl Console {
    pub fn push(&mut self, text: impl AsRef<str>) {
        for line in text.as_ref().split('\n') {
            if self.lines.len() >= layout::CONSOLE_LINES {
                self.lines.pop_front();
            }
            self.lines.push_back(line.to_string());
        }
    }
    pub fn clear(&mut self) {
        self.lines.clear();
    }
}

/// The build in progress as the cards and the status bar show it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BuildProgress {
    pub phase: String,
    pub current: i64,
    pub total: i64,
}
