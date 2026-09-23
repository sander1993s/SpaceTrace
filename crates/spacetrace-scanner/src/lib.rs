//! Metadata-only, streaming filesystem traversal.
//!
//! Directory totals count descendant regular-file logical sizes and deduplicated
//! allocated bytes. Directory entry storage is excluded. Links are recorded but
//! never followed. Allocation for a hard-linked file belongs to its first observed
//! path (native directory order, not sorted order); every path retains its logical
//! size. Memory is O(depth + number of multiply-linked file identities).

use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs::{self, Metadata, ReadDir},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Instant, UNIX_EPOCH},
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Node {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub name: String,
    pub path: String,
    pub kind: String,
    pub logical_bytes: u64,
    pub allocated_bytes: Option<u64>,
    pub files: u64,
    pub directories: u64,
    pub modified: Option<String>,
    pub complete: bool,
    pub shared: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "event",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ScanEvent {
    Node {
        node: Node,
    },
    Issue {
        path: String,
        kind: String,
        message: String,
    },
    Progress {
        entries: u64,
        files: u64,
        directories: u64,
        logical_bytes: u64,
        current_path: String,
    },
    Finished {
        cancelled: bool,
    },
}

struct Frame {
    node: Node,
    children: Option<ReadDir>,
    complete: bool,
}

#[derive(Default)]
struct Stats {
    entries: u64,
    files: u64,
    directories: u64,
    logical_bytes: u64,
}

impl Stats {
    fn event(&self, current_path: String) -> ScanEvent {
        ScanEvent::Progress {
            entries: self.entries,
            files: self.files,
            directories: self.directories,
            logical_bytes: self.logical_bytes,
            current_path,
        }
    }
}

#[derive(Default)]
struct FileDetails {
    allocated: Option<u64>,
    identity: Option<(u64, u128)>,
    reparse_tag: Option<u32>,
    issue: Option<String>,
}

/// Scan a directory without reading file contents or following links.
///
/// Nodes are upserts: each directory appears before its children with
/// `complete=false`, then again with final descendant totals. Parent IDs are
/// smaller than child IDs. Cancellation finalizes open directories with retained
/// subtotals and `complete=false`, then emits Finished. Entry-level failures emit
/// Issue and scanning continues; a bad root or failed event sink returns Err.
pub fn scan(
    root: &Path,
    cancel: &AtomicBool,
    mut emit: impl FnMut(ScanEvent) -> Result<(), String>,
) -> Result<(), String> {
    let initial = fs::symlink_metadata(root)
        .map_err(|e| format!("Cannot inspect {}: {e}", root.display()))?;
    if !initial.is_dir() || initial.file_type().is_symlink() || is_reparse(&initial) {
        return Err("Choose a real directory as the scan root; symbolic links and reparse directories are not followed.".into());
    }
    // On Windows canonicalization supplies the extended-length path prefix,
    // enabling long paths and UNC paths without changing the exported root.
    let absolute_root =
        fs::canonicalize(root).map_err(|e| format!("Cannot resolve {}: {e}", root.display()))?;
    if is_virtual_directory(&absolute_root) {
        return Err(
            "Virtual system directories /proc, /sys, and /dev are excluded from disk-usage scans."
                .into(),
        );
    }
    let root_filesystem = filesystem_id(&initial);
    let name = root
        .file_name()
        .or_else(|| absolute_root.file_name())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.to_string_lossy().into_owned());
    let root_node = Node {
        id: 0,
        parent_id: None,
        name,
        path: ".".into(),
        kind: "directory".into(),
        logical_bytes: 0,
        allocated_bytes: Some(0),
        files: 0,
        directories: 0,
        modified: modified(&initial),
        complete: false,
        shared: false,
    };
    emit(ScanEvent::Node {
        node: root_node.clone(),
    })?;
    let mut stack = vec![open_directory(root_node, absolute_root.clone(), &mut emit)?];
    let mut stats = Stats {
        entries: 1,
        directories: 1,
        ..Stats::default()
    };
    let mut next_id = 1i64;
    let mut seen_hardlinks = HashSet::<(u64, u128)>::new();
    let mut last_progress = Instant::now();
    let mut cancelled = false;
    emit(stats.event(".".into()))?;

    while !stack.is_empty() {
        if cancel.load(Ordering::Relaxed) {
            cancelled = true;
            while !stack.is_empty() {
                finish_directory(&mut stack, true, &mut emit)?;
            }
            break;
        }
        let child = stack
            .last_mut()
            .and_then(|frame| frame.children.as_mut())
            .and_then(Iterator::next);
        let entry = match child {
            None => {
                finish_directory(&mut stack, false, &mut emit)?;
                continue;
            }
            Some(Err(error)) => {
                let frame = stack.last_mut().expect("a frame is active");
                frame.complete = false;
                emit(ScanEvent::Issue {
                    path: frame.node.path.clone(),
                    kind: io_kind(&error).into(),
                    message: format!("Could not enumerate a directory entry: {error}"),
                })?;
                continue;
            }
            Some(Ok(entry)) => entry,
        };
        let absolute = entry.path();
        let relative = relative_path(&absolute_root, &absolute);
        let parent_id = stack.last().map(|frame| frame.node.id);
        let metadata = match fs::symlink_metadata(&absolute) {
            Ok(metadata) => metadata,
            Err(error) => {
                stack.last_mut().expect("a frame is active").complete = false;
                emit(ScanEvent::Issue {
                    path: relative,
                    kind: io_kind(&error).into(),
                    message: error.to_string(),
                })?;
                continue;
            }
        };
        if entry.file_name().to_str().is_none() {
            emit(ScanEvent::Issue {
                path: relative.clone(), kind: "nonUnicodeName".into(),
                message: "This name cannot be represented as Unicode. Its display/export path contains replacement characters; its scan ID remains unique and the original filesystem path is used for scanning.".into(),
            })?;
        }
        let details = file_details(&absolute, &metadata);
        let reparse = is_reparse(&metadata);
        let named_link = metadata.file_type().is_symlink()
            || details
                .reparse_tag
                .is_some_and(|tag| tag & 0x2000_0000 != 0);
        let directory = metadata.is_dir() && !named_link;
        let another_filesystem = directory && filesystem_id(&metadata) != root_filesystem;
        let virtual_directory = directory && is_virtual_directory(&absolute);
        let skip_directory = directory && (reparse || another_filesystem || virtual_directory);
        let regular_file = metadata.is_file() && !named_link;
        let kind = if named_link {
            "link"
        } else if directory {
            "directory"
        } else if regular_file {
            "file"
        } else {
            "special"
        };
        let shared = details.identity.is_some();
        let allocated_bytes = if directory {
            Some(0)
        } else if named_link {
            Some(0)
        } else if let Some(identity) = details.identity {
            if seen_hardlinks.insert(identity) {
                details.allocated
            } else {
                Some(0)
            }
        } else {
            details.allocated
        };
        let node = Node {
            id: next_id,
            parent_id,
            name: entry.file_name().to_string_lossy().into_owned(),
            path: relative.clone(),
            kind: kind.into(),
            logical_bytes: if regular_file { metadata.len() } else { 0 },
            allocated_bytes,
            files: u64::from(regular_file),
            directories: 0,
            modified: modified(&metadata),
            complete: !directory,
            shared,
        };
        next_id = next_id
            .checked_add(1)
            .ok_or("The scan exceeded its supported entry count")?;
        stats.entries = stats.entries.saturating_add(1);
        stats.files = stats.files.saturating_add(node.files);
        stats.directories = stats.directories.saturating_add(u64::from(directory));
        stats.logical_bytes = stats.logical_bytes.saturating_add(node.logical_bytes);
        emit(ScanEvent::Node { node: node.clone() })?;
        if let Some(message) = details.issue {
            emit(ScanEvent::Issue {
                path: relative.clone(),
                kind: "allocationUnavailable".into(),
                message,
            })?;
        }
        if skip_directory {
            emit(ScanEvent::Issue {
                path: relative.clone(),
                kind: if virtual_directory { "virtualDirectorySkipped" } else if another_filesystem { "mountPointSkipped" } else { "reparseDirectorySkipped" }.into(),
                message: if virtual_directory {
                    "Virtual system directories /proc, /sys, and /dev are excluded from disk-usage scans."
                } else if another_filesystem {
                    "This directory is on a different filesystem. Select it as a new scan root to inspect it."
                } else {
                    "This reparse directory was not entered, to avoid following a redirect or downloading cloud-only data."
                }.into(),
            })?;
            let mut skipped = node;
            skipped.allocated_bytes = None;
            emit(ScanEvent::Node {
                node: skipped.clone(),
            })?;
            add_child(stack.last_mut().expect("a frame is active"), &skipped);
        } else if directory {
            stack.push(open_directory(node, absolute, &mut emit)?);
        } else {
            add_child(stack.last_mut().expect("a frame is active"), &node);
        }
        if last_progress.elapsed().as_millis() >= 250 {
            emit_open_directory_snapshots(&stack, &mut emit)?;
            emit(stats.event(relative))?;
            last_progress = Instant::now();
        }
    }
    emit(stats.event(".".into()))?;
    emit(ScanEvent::Finished { cancelled })
}

fn open_directory(
    node: Node,
    absolute: PathBuf,
    emit: &mut impl FnMut(ScanEvent) -> Result<(), String>,
) -> Result<Frame, String> {
    let (children, complete) = match fs::read_dir(&absolute) {
        Ok(children) => (Some(children), true),
        Err(error) => {
            emit(ScanEvent::Issue {
                path: node.path.clone(),
                kind: io_kind(&error).into(),
                message: error.to_string(),
            })?;
            (None, false)
        }
    };
    Ok(Frame {
        node,
        children,
        complete,
    })
}

fn finish_directory(
    stack: &mut Vec<Frame>,
    cancelled: bool,
    emit: &mut impl FnMut(ScanEvent) -> Result<(), String>,
) -> Result<(), String> {
    let mut frame = stack.pop().expect("a frame is active");
    frame.node.complete = frame.complete && !cancelled;
    if let Some(parent) = stack.last_mut() {
        add_child(parent, &frame.node);
    }
    emit(ScanEvent::Node { node: frame.node })
}

fn add_child(parent: &mut Frame, node: &Node) {
    parent.node.logical_bytes = parent.node.logical_bytes.saturating_add(node.logical_bytes);
    parent.node.allocated_bytes = parent
        .node
        .allocated_bytes
        .zip(node.allocated_bytes)
        .map(|(parent, child)| parent.saturating_add(child));
    parent.node.files = parent.node.files.saturating_add(node.files);
    parent.node.directories = parent
        .node
        .directories
        .saturating_add(node.directories)
        .saturating_add(u64::from(node.kind == "directory"));
    parent.complete &= node.complete;
}

fn emit_open_directory_snapshots(
    stack: &[Frame],
    emit: &mut impl FnMut(ScanEvent) -> Result<(), String>,
) -> Result<(), String> {
    // Frames contain only finalized children. Fold the currently open branch
    // into cloned snapshots; committed counters must never see these subtotals.
    let mut active_child: Option<Node> = None;
    for frame in stack.iter().rev() {
        let mut snapshot = frame.node.clone();
        if let Some(child) = active_child {
            snapshot.logical_bytes = snapshot.logical_bytes.saturating_add(child.logical_bytes);
            snapshot.allocated_bytes = snapshot
                .allocated_bytes
                .zip(child.allocated_bytes)
                .map(|(parent, child)| parent.saturating_add(child));
            snapshot.files = snapshot.files.saturating_add(child.files);
            snapshot.directories = snapshot
                .directories
                .saturating_add(child.directories)
                .saturating_add(1);
        }
        snapshot.complete = false;
        emit(ScanEvent::Node {
            node: snapshot.clone(),
        })?;
        active_child = Some(snapshot);
    }
    Ok(())
}

#[cfg(unix)]
fn filesystem_id(metadata: &Metadata) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    Some(metadata.dev())
}

#[cfg(not(unix))]
fn filesystem_id(_metadata: &Metadata) -> Option<u64> {
    None
}

#[cfg(target_os = "linux")]
fn is_virtual_directory(path: &Path) -> bool {
    ["/proc", "/sys", "/dev"]
        .iter()
        .any(|root| path.starts_with(root))
}

#[cfg(not(target_os = "linux"))]
fn is_virtual_directory(_path: &Path) -> bool {
    false
}

fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

fn io_kind(error: &std::io::Error) -> &'static str {
    match error.kind() {
        std::io::ErrorKind::PermissionDenied => "permissionDenied",
        std::io::ErrorKind::NotFound => "disappeared",
        _ => "ioError",
    }
}

fn modified(metadata: &Metadata) -> Option<String> {
    let seconds = metadata
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some(format_timestamp(seconds))
}

fn format_timestamp(seconds: u64) -> String {
    // Gregorian civil date conversion, valid for nonnegative Unix timestamps.
    let days = (seconds / 86_400) as i64 + 719_468;
    let era = days / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_part = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_part + 2) / 5 + 1;
    let month = month_part + if month_part < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        seconds / 3_600 % 24,
        seconds / 60 % 60,
        seconds % 60
    )
}

#[cfg(unix)]
fn file_details(_path: &Path, metadata: &Metadata) -> FileDetails {
    use std::os::unix::fs::MetadataExt;
    FileDetails {
        allocated: Some(metadata.blocks().saturating_mul(512)),
        identity: (metadata.nlink() > 1 && metadata.is_file())
            .then(|| (metadata.dev(), metadata.ino() as u128)),
        ..FileDetails::default()
    }
}

#[cfg(not(any(unix, windows)))]
fn file_details(_path: &Path, _metadata: &Metadata) -> FileDetails {
    FileDetails::default()
}

#[cfg(not(windows))]
fn is_reparse(_metadata: &Metadata) -> bool {
    false
}

#[cfg(windows)]
fn is_reparse(metadata: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes() & 0x400 != 0
}

#[cfg(windows)]
fn file_details(path: &Path, metadata: &Metadata) -> FileDetails {
    use std::{
        ffi::c_void,
        mem::{size_of, zeroed},
        os::windows::{ffi::OsStrExt, fs::MetadataExt},
    };
    type Handle = *mut c_void;
    #[repr(C)]
    struct FileTime {
        low: u32,
        high: u32,
    }
    #[repr(C)]
    struct ByHandleInfo {
        attributes: u32,
        creation: FileTime,
        access: FileTime,
        write: FileTime,
        volume: u32,
        size_high: u32,
        size_low: u32,
        links: u32,
        index_high: u32,
        index_low: u32,
    }
    #[repr(C)]
    struct StandardInfo {
        allocation: i64,
        end_of_file: i64,
        links: u32,
        delete_pending: u8,
        directory: u8,
    }
    #[repr(C)]
    struct AttributeTagInfo {
        attributes: u32,
        tag: u32,
    }
    #[repr(C)]
    struct FileIdInfo {
        volume: u64,
        id: [u8; 16],
    }
    #[repr(C)]
    struct CompressionInfo {
        size: i64,
        format: u16,
        unit_shift: u8,
        chunk_shift: u8,
        cluster_shift: u8,
        reserved: [u8; 3],
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn CreateFileW(
            name: *const u16,
            access: u32,
            share: u32,
            security: *const c_void,
            disposition: u32,
            flags: u32,
            template: Handle,
        ) -> Handle;
        fn GetFileInformationByHandle(handle: Handle, information: *mut ByHandleInfo) -> i32;
        fn GetFileInformationByHandleEx(
            handle: Handle,
            class: i32,
            information: *mut c_void,
            size: u32,
        ) -> i32;
        fn CloseHandle(handle: Handle) -> i32;
    }
    struct OwnedHandle(Handle);
    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
    // Normal directories need no allocation/identity metadata. Reparse-point
    // directories do, so junctions can be distinguished from cloud placeholders.
    if metadata.is_dir() && !is_reparse(metadata) {
        return FileDetails::default();
    }
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // FILE_READ_ATTRIBUTES, share read/write/delete, OPEN_EXISTING,
    // BACKUP_SEMANTICS | OPEN_REPARSE_POINT | OPEN_NO_RECALL.
    // Never requests content access or recall of remotely stored data.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            0x80,
            7,
            std::ptr::null(),
            3,
            0x0230_0000,
            std::ptr::null_mut(),
        )
    };
    if handle == (-1isize) as Handle {
        return FileDetails {
            issue: Some(format!(
                "Allocated size and hard-link identity unavailable: {}",
                std::io::Error::last_os_error()
            )),
            ..FileDetails::default()
        };
    }
    let handle = OwnedHandle(handle);
    let mut details = FileDetails::default();
    unsafe {
        let mut standard: StandardInfo = zeroed();
        if GetFileInformationByHandleEx(
            handle.0,
            1,
            (&mut standard as *mut StandardInfo).cast(),
            size_of::<StandardInfo>() as u32,
        ) != 0
        {
            details.allocated = u64::try_from(standard.allocation).ok();
        } else {
            details.issue = Some(format!(
                "Allocated size unavailable: {}",
                std::io::Error::last_os_error()
            ));
        }
        // For sparse/compressed files the compression information reports the
        // actual storage, whereas standard allocation can describe the stream.
        if metadata.file_attributes() & (0x200 | 0x800) != 0 {
            let mut compression: CompressionInfo = zeroed();
            if GetFileInformationByHandleEx(
                handle.0,
                8,
                (&mut compression as *mut CompressionInfo).cast(),
                size_of::<CompressionInfo>() as u32,
            ) != 0
            {
                details.allocated = u64::try_from(compression.size).ok();
            } else {
                details.allocated = None;
                details.issue = Some(format!(
                    "Sparse/compressed allocated size unavailable: {}",
                    std::io::Error::last_os_error()
                ));
            }
        }
        // StandardInfo supplies the link count, so ordinary files need no
        // identity query (particularly valuable on network shares).
        if standard.links > 1 && metadata.is_file() {
            let mut file_id: FileIdInfo = zeroed();
            if GetFileInformationByHandleEx(
                handle.0,
                18,
                (&mut file_id as *mut FileIdInfo).cast(),
                size_of::<FileIdInfo>() as u32,
            ) != 0
            {
                details.identity = Some((file_id.volume, u128::from_ne_bytes(file_id.id)));
            } else {
                let mut information: ByHandleInfo = zeroed();
                if GetFileInformationByHandle(handle.0, &mut information) != 0 {
                    details.identity = Some((
                        information.volume as u64,
                        ((information.index_high as u64) << 32 | information.index_low as u64)
                            as u128,
                    ));
                } else {
                    details.allocated = None;
                    details.issue = Some(format!(
                        "Allocated size withheld because hard-link identity could not be verified: {}",
                        std::io::Error::last_os_error()
                    ));
                }
            }
        }
        if is_reparse(metadata) {
            let mut attributes: AttributeTagInfo = zeroed();
            if GetFileInformationByHandleEx(
                handle.0,
                9,
                (&mut attributes as *mut AttributeTagInfo).cast(),
                size_of::<AttributeTagInfo>() as u32,
            ) != 0
            {
                details.reparse_tag = Some(attributes.tag);
            }
        }
    }
    details
}
