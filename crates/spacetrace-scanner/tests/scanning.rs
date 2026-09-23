use spacetrace_scanner::{scan, Node, ScanEvent};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let folder = std::env::temp_dir().join(format!(
            "spacetrace-scanner-test-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&folder).unwrap();
        Self(folder)
    }
    fn path(&self) -> &Path {
        &self.0
    }
    fn file(&self, name: &str, bytes: usize) {
        let path = self.0.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, vec![b'x'; bytes]).unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn collect(root: &Path) -> Vec<ScanEvent> {
    let mut events = Vec::new();
    scan(root, &AtomicBool::new(false), |event| {
        events.push(event);
        Ok(())
    })
    .unwrap();
    events
}

fn nodes(events: &[ScanEvent]) -> HashMap<i64, Node> {
    events
        .iter()
        .filter_map(|event| match event {
            ScanEvent::Node { node } => Some((node.id, node.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn exact_recursive_totals_and_directory_upserts() {
    let fixture = Fixture::new();
    fixture.file("one.bin", 7);
    fixture.file("nested/two.bin", 13);
    fixture.file("nested/deeper/three.bin", 19);
    fixture.file("nested/deeper/empty.bin", 0);
    fs::create_dir(fixture.path().join("empty-folder")).unwrap();
    let events = collect(fixture.path());
    let final_nodes = nodes(&events);
    let root = &final_nodes[&0];
    assert_eq!(
        (root.logical_bytes, root.files, root.directories),
        (39, 4, 3)
    );
    assert!(root.complete);
    assert_eq!(root.path, ".");
    assert_eq!(root.parent_id, None);
    assert_eq!(final_nodes.len(), 8);
    for node in final_nodes.values() {
        if let Some(parent) = node.parent_id {
            assert!(parent < node.id);
        }
        if node.kind == "directory" {
            let upserts: Vec<&Node> = events
                .iter()
                .filter_map(|event| match event {
                    ScanEvent::Node { node: candidate } if candidate.id == node.id => {
                        Some(candidate)
                    }
                    _ => None,
                })
                .collect();
            assert!(upserts.len() >= 2);
            assert!(!upserts[0].complete);
            assert!(upserts.last().unwrap().complete);
        }
    }
    let final_progress = events
        .iter()
        .rev()
        .find_map(|event| match event {
            ScanEvent::Progress {
                entries,
                files,
                directories,
                logical_bytes,
                ..
            } => Some((*entries, *files, *directories, *logical_bytes)),
            _ => None,
        })
        .unwrap();
    assert_eq!(final_progress, (8, 4, 4, 39));
    assert!(matches!(
        events.last(),
        Some(ScanEvent::Finished { cancelled: false })
    ));
}

#[test]
fn cancellation_preserves_subtotals_and_finalizes_ancestors() {
    let fixture = Fixture::new();
    for n in 0..25 {
        fixture.file(&format!("nested/item-{n}.bin"), 31);
    }
    let cancel = AtomicBool::new(false);
    let mut events = Vec::new();
    let mut files_seen = 0;
    scan(fixture.path(), &cancel, |event| {
        if matches!(&event, ScanEvent::Node { node } if node.kind == "file") {
            files_seen += 1;
            if files_seen == 3 {
                cancel.store(true, Ordering::Relaxed);
            }
        }
        events.push(event);
        Ok(())
    })
    .unwrap();
    let final_nodes = nodes(&events);
    let root = &final_nodes[&0];
    assert_eq!((root.logical_bytes, root.files), (93, 3));
    assert!(!root.complete);
    assert!(final_nodes
        .values()
        .filter(|n| n.kind == "directory")
        .all(|n| !n.complete));
    assert!(matches!(
        events.last(),
        Some(ScanEvent::Finished { cancelled: true })
    ));
}

#[test]
fn partial_ancestor_totals_are_visible_and_never_double_counted() {
    let fixture = Fixture::new();
    fixture.file("nested/deep/one.bin", 17);
    fixture.file("nested/deep/two.bin", 23);
    let mut events = Vec::new();
    let mut slowed_once = false;
    scan(fixture.path(), &AtomicBool::new(false), |event| {
        if !slowed_once && matches!(&event, ScanEvent::Node { node } if node.kind == "file") {
            std::thread::sleep(std::time::Duration::from_millis(275));
            slowed_once = true;
        }
        events.push(event);
        Ok(())
    })
    .unwrap();
    let partials: Vec<&Node> = events
        .iter()
        .filter_map(|event| match event {
            ScanEvent::Node { node }
                if node.id == 0 && !node.complete && node.logical_bytes > 0 =>
            {
                Some(node)
            }
            _ => None,
        })
        .collect();
    assert!(!partials.is_empty());
    assert!(partials[0].logical_bytes == 17 || partials[0].logical_bytes == 23);
    assert_eq!((partials[0].files, partials[0].directories), (1, 2));
    let final_nodes = nodes(&events);
    assert_eq!(
        (
            final_nodes[&0].logical_bytes,
            final_nodes[&0].files,
            final_nodes[&0].directories
        ),
        (40, 2, 2)
    );
    assert!(final_nodes[&0].complete);
}

#[test]
fn pre_cancelled_scan_emits_empty_incomplete_root() {
    let fixture = Fixture::new();
    fixture.file("unseen.bin", 20);
    let mut events = Vec::new();
    scan(fixture.path(), &AtomicBool::new(true), |event| {
        events.push(event);
        Ok(())
    })
    .unwrap();
    let final_nodes = nodes(&events);
    assert_eq!(final_nodes.len(), 1);
    assert_eq!(final_nodes[&0].logical_bytes, 0);
    assert!(!final_nodes[&0].complete);
}

#[test]
fn hardlinks_keep_each_logical_size_and_count_allocation_once() {
    let fixture = Fixture::new();
    fixture.file("original.bin", 12_345);
    fs::hard_link(
        fixture.path().join("original.bin"),
        fixture.path().join("alias.bin"),
    )
    .unwrap();
    let final_nodes = nodes(&collect(fixture.path()));
    let root = &final_nodes[&0];
    assert_eq!((root.logical_bytes, root.files), (24_690, 2));
    let files: Vec<&Node> = final_nodes.values().filter(|n| n.kind == "file").collect();
    assert!(files.iter().all(|n| n.shared));
    let allocations: Vec<u64> = files.iter().map(|n| n.allocated_bytes.unwrap()).collect();
    assert_eq!(allocations.iter().filter(|&&n| n == 0).count(), 1);
    assert_eq!(root.allocated_bytes, Some(allocations.iter().sum()));
    assert!(root.allocated_bytes.unwrap() > 0);
}

#[test]
fn disappearing_directory_is_an_issue_and_marks_root_incomplete() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.path().join("gone")).unwrap();
    let mut events = Vec::new();
    scan(fixture.path(), &AtomicBool::new(false), |event| {
        if matches!(&event, ScanEvent::Node { node } if node.path == "gone" && !node.complete) {
            let _ = fs::remove_dir(fixture.path().join("gone"));
        }
        events.push(event);
        Ok(())
    })
    .unwrap();
    assert!(!nodes(&events)[&0].complete);
    assert!(events.iter().any(|e| matches!(e, ScanEvent::Issue { path, kind, .. } if path == "gone" && kind == "disappeared")));
    assert!(matches!(
        events.last(),
        Some(ScanEvent::Finished { cancelled: false })
    ));
}

#[cfg(windows)]
#[test]
fn sparse_file_reports_physical_allocation_instead_of_logical_length() {
    use std::{
        ffi::c_void,
        io::{Seek, SeekFrom, Write},
        os::windows::io::AsRawHandle,
    };
    #[link(name = "kernel32")]
    extern "system" {
        fn DeviceIoControl(
            handle: *mut c_void,
            code: u32,
            input: *const c_void,
            input_size: u32,
            output: *mut c_void,
            output_size: u32,
            returned: *mut u32,
            overlapped: *mut c_void,
        ) -> i32;
    }
    let fixture = Fixture::new();
    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(fixture.path().join("sparse.bin"))
        .unwrap();
    let mut returned = 0;
    let sparse = unsafe {
        DeviceIoControl(
            file.as_raw_handle(),
            0x0009_00c4,
            std::ptr::null(),
            0,
            std::ptr::null_mut(),
            0,
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    if sparse == 0 {
        let error = std::io::Error::last_os_error();
        if matches!(error.raw_os_error(), Some(1 | 50)) {
            return;
        }
        panic!("could not mark fixture sparse: {error}");
    }
    let length = 8 * 1024 * 1024;
    file.set_len(length).unwrap();
    file.seek(SeekFrom::Start(length - 4)).unwrap();
    file.write_all(b"tail").unwrap();
    file.sync_all().unwrap();
    drop(file);
    let final_nodes = nodes(&collect(fixture.path()));
    assert_eq!(final_nodes[&0].logical_bytes, length);
    let allocated = final_nodes[&0].allocated_bytes.unwrap();
    assert!(
        allocated > 0 && allocated < length,
        "unexpected sparse allocation: {allocated}"
    );
}

#[test]
fn sink_failure_stops_scanning_without_success_event() {
    let fixture = Fixture::new();
    fixture.file("one", 1);
    let mut calls = 0;
    let error = scan(fixture.path(), &AtomicBool::new(false), |_| {
        calls += 1;
        if calls == 3 {
            Err("sink disconnected".into())
        } else {
            Ok(())
        }
    })
    .unwrap_err();
    assert_eq!(error, "sink disconnected");
    assert_eq!(calls, 3);
}

#[test]
fn rejects_non_directory_roots() {
    let fixture = Fixture::new();
    fixture.file("file", 3);
    assert!(scan(
        &fixture.path().join("file"),
        &AtomicBool::new(false),
        |_| Ok(())
    )
    .is_err());
    assert!(scan(
        &fixture.path().join("missing"),
        &AtomicBool::new(false),
        |_| Ok(())
    )
    .is_err());
}

#[test]
fn deep_directories_and_long_paths_preserve_totals() {
    let fixture = Fixture::new();
    // Canonicalize early to use Windows' extended-length path prefix while
    // creating this fixture, independently of machine long-path policy.
    let mut deepest = fs::canonicalize(fixture.path()).unwrap();
    for _ in 0..80 {
        deepest.push("nested-folder");
        fs::create_dir(&deepest).unwrap();
    }
    fs::write(deepest.join("bottom.bin"), b"bottom").unwrap();
    let final_nodes = nodes(&collect(fixture.path()));
    assert_eq!(
        (final_nodes[&0].logical_bytes, final_nodes[&0].directories),
        (6, 80)
    );
    assert!(final_nodes[&0].complete);
}

#[test]
fn protocol_round_trips_camel_case_fields_and_unicode_paths() {
    let fixture = Fixture::new();
    fixture.file("rêves/星.bin", 10);
    let events = collect(fixture.path());
    for event in &events {
        let json = serde_json::to_string(event).unwrap();
        let _: ScanEvent = serde_json::from_str(&json).unwrap();
        assert!(!json.contains("logical_bytes"));
        assert!(!json.contains("parent_id"));
        assert!(!json.contains("current_path"));
    }
    assert!(nodes(&events).values().any(|n| n.path == "rêves/星.bin"));
    let json = serde_json::to_value(
        events
            .iter()
            .find(|e| matches!(e, ScanEvent::Progress { .. }))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(json["event"], "progress");
    assert_eq!(json["currentPath"], ".");
}

#[test]
fn symbolic_link_cycles_are_not_followed() {
    let fixture = Fixture::new();
    fixture.file("content/file", 27);
    #[cfg(unix)]
    let result = std::os::unix::fs::symlink(fixture.path(), fixture.path().join("cycle"));
    #[cfg(windows)]
    let result = std::os::windows::fs::symlink_dir(fixture.path(), fixture.path().join("cycle"));
    #[cfg(not(any(unix, windows)))]
    return;
    #[cfg(any(unix, windows))]
    {
        if let Err(error) = result {
            // Windows restricts symlink creation unless Developer Mode or the
            // relevant privilege is enabled. All other failures are test errors.
            if error.kind() == std::io::ErrorKind::PermissionDenied
                || error.raw_os_error() == Some(1314)
            {
                return;
            }
            panic!("could not create test symlink: {error}");
        }
        let final_nodes = nodes(&collect(fixture.path()));
        assert_eq!(
            (final_nodes[&0].logical_bytes, final_nodes[&0].files),
            (27, 1)
        );
        let link = final_nodes.values().find(|n| n.path == "cycle").unwrap();
        assert_eq!(link.kind, "link");
        assert_eq!(link.logical_bytes, 0);
        assert!(scan(
            &fixture.path().join("cycle"),
            &AtomicBool::new(false),
            |_| Ok(())
        )
        .is_err());
    }
}

#[cfg(unix)]
#[test]
fn invalid_unicode_names_are_scanned_and_explicitly_reported() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    let fixture = Fixture::new();
    fs::write(
        fixture.path().join(OsString::from_vec(vec![b'f', 0xff])),
        b"hello",
    )
    .unwrap();
    let events = collect(fixture.path());
    assert_eq!(nodes(&events)[&0].logical_bytes, 5);
    assert!(events
        .iter()
        .any(|e| matches!(e, ScanEvent::Issue { kind, .. } if kind == "nonUnicodeName")));
}
