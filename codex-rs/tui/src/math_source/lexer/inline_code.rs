//! Backtick indexing with conservative paragraph boundaries.

use std::collections::HashMap;

use super::byte_run;
use super::fence_marker;
use super::html;
use super::indent::list_marker;
use super::source_lines;
use super::strip_quote_prefixes;

pub(super) struct Backticks {
    runs: HashMap<usize, Vec<usize>>,
    boundaries: Vec<usize>,
}

impl Backticks {
    pub(super) fn new(source: &str) -> Self {
        let bytes = source.as_bytes();
        let mut runs: HashMap<usize, Vec<usize>> = HashMap::new();
        let mut cursor = 0;
        while cursor < bytes.len() {
            if bytes[cursor] != b'`' {
                cursor += 1;
                continue;
            }
            let len = byte_run(&bytes[cursor..], b'`');
            runs.entry(len).or_default().push(cursor);
            cursor += len;
        }
        let boundaries = source_lines(source)
            .filter(|line| {
                let content = strip_quote_prefixes(line.text, usize::MAX).0.trim_start();
                content.is_empty()
                    || fence_marker(content).is_some()
                    || list_marker(content, /*marker_column*/ 0).is_some_and(|marker| {
                        fence_marker(&content[marker.content_bytes..]).is_some()
                    })
                    || html::block_start(content).is_some()
            })
            .map(|line| line.start)
            .collect();
        Self { runs, boundaries }
    }

    pub(super) fn next(&self, opener: usize, len: usize) -> Option<usize> {
        let runs = self.runs.get(&len)?;
        let closer = runs
            .get(runs.partition_point(|position| *position <= opener))
            .copied()?;
        let boundary = self
            .boundaries
            .get(
                self.boundaries
                    .partition_point(|position| *position <= opener),
            )
            .copied();
        boundary
            .is_none_or(|boundary| boundary > closer)
            .then_some(closer)
    }
}
