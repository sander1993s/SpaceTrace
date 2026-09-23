//! Behavioral fixtures for interrupted scans. These represent records already
//! received from a helper, without assuming a final progress/root event arrives.

use crate::{
    models::{NodeInfo, ScanInfo, ScanRequest},
    store,
};
use rusqlite::Connection;
use std::path::PathBuf;

const SCAN: &str = "interrupted-fixture";

struct Fixture {
    directory: tempfile::TempDir,
    db: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let db = directory.path().join("scans.db");
        store::initialize(&db).unwrap();
        let conn = store::connect(&db).unwrap();
        store::save_scan(&conn, &scan_info(SCAN)).unwrap();
        Self { directory, db }
    }

    fn connect(&self) -> Connection {
        store::connect(&self.db).unwrap()
    }
}

fn scan_info(id: &str) -> ScanInfo {
    ScanInfo {
        id: id.into(),
        source: ScanRequest {
            kind: "ssh".into(),
            root: "/fixture".into(),
            host: Some("fixture.invalid".into()),
            port: Some(22),
            username: Some("fixture-user".into()),
            key_path: None,
            platform: Some("linux".into()),
        },
        status: "scanning".into(),
        started_at: "2026-01-01T00:00:00Z".into(),
        finished_at: None,
        root_name: "fixture".into(),
        entries: 0,
        files: 0,
        directories: 0,
        logical_bytes: "0".into(),
        allocated_bytes: Some("0".into()),
        issue_count: 0,
        current_path: "assets".into(),
        error: None,
    }
}

fn directory(id: i64, parent_id: Option<i64>, path: &str) -> NodeInfo {
    NodeInfo {
        id,
        parent_id,
        name: if path == "." {
            "fixture"
        } else {
            path.rsplit('/').next().unwrap()
        }
        .into(),
        path: path.into(),
        kind: "directory".into(),
        logical_bytes: "0".into(),
        allocated_bytes: Some("0".into()),
        files: 0,
        directories: 0,
        modified: Some("2025-12-31T23:59:00Z".into()),
        complete: false,
        shared: false,
    }
}

fn file(id: i64, parent: i64, path: &str, logical: u64, allocated: Option<u64>) -> NodeInfo {
    let mut node = directory(id, Some(parent), path);
    node.kind = "file".into();
    node.logical_bytes = logical.to_string();
    node.allocated_bytes = allocated.map(|bytes| bytes.to_string());
    node.files = 1;
    node.complete = true;
    node
}

fn persist(conn: &Connection, nodes: &[NodeInfo]) {
    for node in nodes {
        store::save_node(conn, SCAN, node).unwrap();
    }
}

#[test]
fn reconcile_keeps_nested_file_received_without_a_progress_event() {
    let fixture = Fixture::new();
    let conn = fixture.connect();
    persist(
        &conn,
        &[
            directory(0, None, "."),
            directory(1, Some(0), "project"),
            directory(2, Some(1), "project/assets"),
            file(
                3,
                2,
                "project/assets/image.bin",
                104_857_601,
                Some(104_861_696),
            ),
        ],
    );
    assert_eq!(store::node(&conn, SCAN, 0).unwrap().logical_bytes, "0");

    let root = store::reconcile_partial(&conn, SCAN).unwrap();
    assert_eq!(root.logical_bytes, "104857601");
    assert_eq!(root.allocated_bytes.as_deref(), Some("104861696"));
    assert_eq!((root.files, root.directories), (1, 2));
    assert!(!root.complete);
    let project = store::node(&conn, SCAN, 1).unwrap();
    assert_eq!(project.logical_bytes, "104857601");
    assert_eq!((project.files, project.directories), (1, 1));
    assert!(!project.complete);
    let assets = store::node(&conn, SCAN, 2).unwrap();
    assert_eq!((assets.files, assets.directories), (1, 0));
    assert_eq!(assets.allocated_bytes.as_deref(), Some("104861696"));
    assert!(store::node(&conn, SCAN, 3).unwrap().complete);

    let browser = store::browse(&conn, SCAN, 0, "", 0, 50, "logical").unwrap();
    assert_eq!(browser.children.len(), 1);
    assert_eq!(browser.children[0].logical_bytes, "104857601");
}

#[test]
fn reconcile_does_not_add_completed_branch_totals_twice() {
    let fixture = Fixture::new();
    let conn = fixture.connect();
    let mut root = directory(0, None, ".");
    root.logical_bytes = "30".into();
    root.allocated_bytes = Some("8192".into());
    root.files = 2;
    root.directories = 1;
    let mut archive = directory(1, Some(0), "archive");
    archive.logical_bytes = "30".into();
    archive.allocated_bytes = Some("8192".into());
    archive.files = 2;
    archive.complete = true;
    let original_archive = serde_json::to_value(&archive).unwrap();
    persist(
        &conn,
        &[
            root,
            archive,
            file(2, 1, "archive/one.bin", 11, Some(4096)),
            file(3, 1, "archive/two.bin", 19, Some(4096)),
            directory(4, Some(0), "current"),
            directory(5, Some(4), "current/nested"),
            file(6, 5, "current/nested/three.bin", 7, Some(4096)),
        ],
    );

    let first = store::reconcile_partial(&conn, SCAN).unwrap();
    assert_eq!(first.logical_bytes, "37");
    assert_eq!(first.allocated_bytes.as_deref(), Some("12288"));
    assert_eq!((first.files, first.directories), (3, 3));
    assert_eq!(
        serde_json::to_value(store::node(&conn, SCAN, 1).unwrap()).unwrap(),
        original_archive
    );
    assert!(!store::node(&conn, SCAN, 4).unwrap().complete);

    // A second reconciliation can happen after another restart; it must leave
    // the observed data identical rather than accumulating previous subtotals.
    let second = store::reconcile_partial(&conn, SCAN).unwrap();
    assert_eq!(
        serde_json::to_value(second).unwrap(),
        serde_json::to_value(first).unwrap()
    );
}

#[test]
fn unavailable_file_allocation_propagates_without_losing_logical_sizes() {
    let fixture = Fixture::new();
    let conn = fixture.connect();
    persist(
        &conn,
        &[
            directory(0, None, "."),
            directory(1, Some(0), "mixed"),
            file(2, 1, "mixed/unknown.bin", 9, None),
            file(3, 1, "mixed/known.bin", 3, Some(4096)),
        ],
    );

    let root = store::reconcile_partial(&conn, SCAN).unwrap();
    assert_eq!(root.logical_bytes, "12");
    assert_eq!(root.allocated_bytes, None);
    assert_eq!((root.files, root.directories), (2, 1));
    let mixed = store::node(&conn, SCAN, 1).unwrap();
    assert_eq!(mixed.allocated_bytes, None);
    assert_eq!(mixed.logical_bytes, "12");
    assert_eq!(
        store::node(&conn, SCAN, 3)
            .unwrap()
            .allocated_bytes
            .as_deref(),
        Some("4096")
    );
}

#[test]
fn skipped_directory_allocation_stays_unknown_even_with_no_child_rows() {
    let fixture = Fixture::new();
    let conn = fixture.connect();
    let mut skipped = directory(1, Some(0), "cloud-only");
    skipped.allocated_bytes = None;
    persist(
        &conn,
        &[
            directory(0, None, "."),
            skipped,
            file(2, 0, "known.bin", 5, Some(4096)),
        ],
    );

    let root = store::reconcile_partial(&conn, SCAN).unwrap();
    assert_eq!(root.logical_bytes, "5");
    assert_eq!(root.allocated_bytes, None);
    assert_eq!((root.files, root.directories), (1, 1));
    let skipped = store::node(&conn, SCAN, 1).unwrap();
    assert_eq!(skipped.allocated_bytes, None);
    assert!(!skipped.complete);
}

#[test]
fn directory_counts_exclude_self_and_do_not_count_link_entries() {
    let fixture = Fixture::new();
    let conn = fixture.connect();
    let mut empty = directory(1, Some(0), "empty");
    empty.complete = true;
    let mut link = directory(5, Some(2), "branch/loop");
    link.kind = "link".into();
    link.complete = true;
    persist(
        &conn,
        &[
            directory(0, None, "."),
            empty,
            directory(2, Some(0), "branch"),
            directory(3, Some(2), "branch/deep"),
            file(4, 3, "branch/deep/data.bin", 5, Some(4096)),
            link,
            file(6, 0, "top.bin", 3, Some(4096)),
        ],
    );

    let root = store::reconcile_partial(&conn, SCAN).unwrap();
    assert_eq!(
        (root.logical_bytes.as_str(), root.files, root.directories),
        ("8", 2, 3)
    );
    assert_eq!(store::node(&conn, SCAN, 1).unwrap().directories, 0);
    assert_eq!(store::node(&conn, SCAN, 2).unwrap().directories, 1);
    assert_eq!(store::node(&conn, SCAN, 3).unwrap().directories, 0);
    assert_eq!(store::node(&conn, SCAN, 5).unwrap().kind, "link");
}

#[test]
fn reconciling_an_empty_root_has_zero_descendant_directories() {
    let fixture = Fixture::new();
    let conn = fixture.connect();
    persist(&conn, &[directory(0, None, ".")]);
    let root = store::reconcile_partial(&conn, SCAN).unwrap();
    assert_eq!(
        (root.logical_bytes.as_str(), root.files, root.directories),
        ("0", 0, 0)
    );
    assert_eq!(root.allocated_bytes.as_deref(), Some("0"));
    assert!(!root.complete);
}

#[test]
fn startup_recovers_committed_nodes_after_a_crash_and_exports_updated_totals() {
    let fixture = Fixture::new();
    let finished_before;
    {
        let conn = fixture.connect();
        // The last ScanInfo still reports zero because no progress event was
        // received after the helper emitted this directory and file.
        persist(
            &conn,
            &[
                directory(0, None, "."),
                directory(1, Some(0), "assets"),
                file(2, 1, "assets/saved.bin", 9, Some(4096)),
            ],
        );
        let mut finished = scan_info("already-completed");
        finished.status = "completed".into();
        finished.finished_at = Some("2026-01-01T00:00:05Z".into());
        finished.entries = 1;
        finished.current_path.clear();
        store::save_scan(&conn, &finished).unwrap();
        let mut completed_root = directory(0, None, ".");
        completed_root.complete = true;
        store::save_node(&conn, &finished.id, &completed_root).unwrap();
        finished_before = serde_json::to_value(finished).unwrap();
    }

    store::initialize(&fixture.db).unwrap();
    let conn = fixture.connect();
    let recovered = store::get_scan(&conn, SCAN).unwrap();
    assert_eq!(recovered.status, "failed");
    assert!(recovered.finished_at.is_some());
    assert!(recovered
        .error
        .as_deref()
        .unwrap_or("")
        .contains("interrupted"));
    assert_eq!(recovered.logical_bytes, "9");
    assert_eq!(recovered.allocated_bytes.as_deref(), Some("4096"));
    assert_eq!(
        (recovered.entries, recovered.files, recovered.directories),
        (3, 1, 1)
    );
    let root = store::node(&conn, SCAN, 0).unwrap();
    assert_eq!(root.logical_bytes, "9");
    assert_eq!((root.files, root.directories), (1, 1));
    assert!(!root.complete);
    assert_eq!(
        serde_json::to_value(store::get_scan(&conn, "already-completed").unwrap()).unwrap(),
        finished_before
    );
    assert!(store::node(&conn, "already-completed", 0).unwrap().complete);

    let report = fixture.directory.path().join("recovered.json");
    store::export(&fixture.db, SCAN, &report).unwrap();
    let json: serde_json::Value = serde_json::from_slice(&std::fs::read(report).unwrap()).unwrap();
    assert_eq!(json["scanStatus"], "failed");
    assert_eq!(json["isPartial"], true);
    assert_eq!(json["totals"]["logicalBytes"], "9");
    assert_eq!(json["totals"]["directories"], 1);
    assert_eq!(json["nodes"].as_array().unwrap().len(), 3);

    let recovered_before = serde_json::to_value(recovered).unwrap();
    drop(conn);
    store::initialize(&fixture.db).unwrap();
    let conn = fixture.connect();
    assert_eq!(
        serde_json::to_value(store::get_scan(&conn, SCAN).unwrap()).unwrap(),
        recovered_before
    );
}
