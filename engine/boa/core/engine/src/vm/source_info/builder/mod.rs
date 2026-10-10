use std::ops::Range;

use boa_ast::Position;
use itertools::Itertools;

use crate::vm::source_info::Entry;

#[cfg(test)]
mod tests;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct EntryRange {
    start: u32,
    end: u32,
    position: Option<Position>,
}

impl EntryRange {
    fn range(&self) -> Range<u32> {
        self.start..self.end
    }

    fn is_empty(&self) -> bool {
        self.range().is_empty()
    }
}

#[derive(Debug, Default)]
pub(crate) struct SourceMapBuilder {
    entries: Vec<EntryRange>,
    stack: Vec<SourceScope>,
}

/// The whole scope's start is distinct from its latest range after nested
/// expressions restore the enclosing position.
#[derive(Debug, Clone, Copy)]
struct SourceScope {
    start: u32,
    entry: usize,
}

impl SourceMapBuilder {
    pub(crate) fn build(self, final_pc: u32) -> Box<[Entry]> {
        assert!(self.stack.is_empty(), "forgot to pop source scope");
        let end_entry = self
            .entries
            .last()
            .copied()
            .map(|entry| EntryRange {
                start: entry.end,
                end: final_pc,
                position: None,
            })
            .unwrap_or_default();

        self.entries
            .into_iter()
            .chain(std::iter::once(end_entry))
            .filter(|entry| !entry.is_empty())
            .dedup_by(|a, b| a.position == b.position)
            .map(|entry| Entry {
                pc: entry.start,
                position: entry.position,
            })
            .collect::<Box<[_]>>()
    }

    pub(crate) fn push_source_position(&mut self, start_pc: u32, position: Option<Position>) {
        let index = self.entries.len();
        self.entries.push(EntryRange {
            start: start_pc,
            end: u32::MAX,
            position,
        });
        self.stack.push(SourceScope {
            start: start_pc,
            entry: index,
        });
    }

    // TODO: document implementation range flattening.
    pub(crate) fn pop_source_position(&mut self, current_start_pc: u32) {
        let Some(scope) = self.stack.pop() else {
            panic!("popped more than pushed");
        };

        self.entries[scope.entry].end = current_start_pc;

        if scope.start == current_start_pc {
            return;
        }

        let Some(parent) = self.stack.last_mut() else {
            return;
        };
        let range = &mut self.entries[parent.entry];
        assert_eq!(range.end, u32::MAX, "parent source scope must remain open");
        // Exclude the entire child scope, not just its final range. Otherwise
        // grandchildren leave overlapping ranges and erase the throw position.
        range.end = scope.start;
        let position = range.position;
        parent.entry = self.entries.len();
        self.entries.push(EntryRange {
            start: current_start_pc,
            end: u32::MAX,
            position,
        });
    }
}
