//! Latest-change tracking for directories while scan events stream past.
//!
//! A directory's own `modified` time only reflects changes to its entry list.
//! `newest_modified` is the latest modification time of the directory or any
//! descendant, which is what reveals folders that nobody has touched in months.
//! Both local and SSH scans emit directories before their children and emit a
//! directory's final record after all descendants, so folding each timestamp into
//! the open ancestor chain gives final records their complete value.

use crate::models::NodeInfo;
use chrono::{DateTime, SecondsFormat, Utc};
use std::collections::HashMap;

/// Keeps one parent link and one timestamp per directory (about 32 bytes each).
#[derive(Default)]
pub struct NewestTracker {
    directories: HashMap<i64, (Option<i64>, Option<i64>)>,
}

impl NewestTracker {
    /// Records the node's timestamp and, for directories, fills `newest_modified`.
    pub fn observe(&mut self, node: &mut NodeInfo) {
        let stamp = node.modified.as_deref().and_then(epoch_seconds);
        if node.kind == "directory" {
            self.directories
                .entry(node.id)
                .or_insert((node.parent_id, None));
            if let Some(stamp) = stamp {
                self.raise(node.id, stamp);
            }
            node.newest_modified = self
                .directories
                .get(&node.id)
                .and_then(|(_, newest)| *newest)
                .and_then(format_seconds);
        } else if let (Some(parent), Some(stamp)) = (node.parent_id, stamp) {
            self.raise(parent, stamp);
        }
    }

    // Ancestors always hold at least their descendants' value, so the walk stops
    // at the first ancestor that is already as recent.
    fn raise(&mut self, mut id: i64, stamp: i64) {
        while let Some((parent, newest)) = self.directories.get_mut(&id) {
            if newest.is_some_and(|value| value >= stamp) {
                break;
            }
            *newest = Some(stamp);
            match *parent {
                Some(next) => id = next,
                None => break,
            }
        }
    }
}

/// Parses the RFC 3339 timestamps produced by the local scanner and SSH helpers.
pub fn epoch_seconds(value: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|time| time.timestamp())
}

/// Formats like the local scanner: UTC with second precision.
pub fn format_seconds(seconds: i64) -> Option<String> {
    DateTime::<Utc>::from_timestamp(seconds, 0)
        .map(|time| time.to_rfc3339_opts(SecondsFormat::Secs, true))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: i64, parent_id: Option<i64>, kind: &str, modified: Option<&str>) -> NodeInfo {
        NodeInfo {
            id,
            parent_id,
            name: format!("node{id}"),
            path: if id == 0 {
                ".".into()
            } else {
                format!("node{id}")
            },
            kind: kind.into(),
            logical_bytes: "0".into(),
            allocated_bytes: Some("0".into()),
            files: 0,
            directories: 0,
            modified: modified.map(Into::into),
            complete: false,
            shared: false,
            newest_modified: None,
        }
    }

    #[test]
    fn final_directory_records_carry_the_newest_descendant_change() {
        let mut tracker = NewestTracker::default();
        let mut root = node(0, None, "directory", Some("2020-01-01T00:00:00Z"));
        tracker.observe(&mut root);
        assert_eq!(
            root.newest_modified.as_deref(),
            Some("2020-01-01T00:00:00Z")
        );
        let mut old = node(1, Some(0), "directory", Some("2021-01-01T00:00:00Z"));
        tracker.observe(&mut old);
        tracker.observe(&mut node(2, Some(1), "file", Some("2022-06-01T12:00:00Z")));
        let mut recent = node(3, Some(0), "directory", Some("2021-01-01T00:00:00Z"));
        tracker.observe(&mut recent);
        // The SSH Windows helper reports fractional seconds; Linux reports offsets.
        tracker.observe(&mut node(
            4,
            Some(3),
            "file",
            Some("2026-10-16T09:57:33.1234567Z"),
        ));
        tracker.observe(&mut node(
            5,
            Some(3),
            "link",
            Some("2026-10-16T11:57:34+02:00"),
        ));

        // Final records are emitted after their descendants.
        tracker.observe(&mut recent);
        tracker.observe(&mut old);
        tracker.observe(&mut root);
        assert_eq!(old.newest_modified.as_deref(), Some("2022-06-01T12:00:00Z"));
        assert_eq!(
            recent.newest_modified.as_deref(),
            Some("2026-10-16T09:57:34Z")
        );
        assert_eq!(
            root.newest_modified.as_deref(),
            Some("2026-10-16T09:57:34Z")
        );
    }

    #[test]
    fn missing_and_unparsable_timestamps_are_ignored() {
        let mut tracker = NewestTracker::default();
        let mut root = node(0, None, "directory", None);
        tracker.observe(&mut root);
        tracker.observe(&mut node(1, Some(0), "file", Some("not a time")));
        tracker.observe(&mut node(2, Some(0), "file", None));
        tracker.observe(&mut root);
        assert_eq!(root.newest_modified, None);

        let mut file = node(3, Some(0), "file", Some("2024-01-01T00:00:00Z"));
        tracker.observe(&mut file);
        assert_eq!(
            file.newest_modified, None,
            "only directories carry the field"
        );
        tracker.observe(&mut root);
        assert_eq!(
            root.newest_modified.as_deref(),
            Some("2024-01-01T00:00:00Z")
        );
    }
}
