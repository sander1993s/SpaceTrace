mod models;
mod remote;
mod store;

use models::*;
use spacetrace_scanner::ScanEvent;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tauri::{Manager, State};

#[derive(Clone)]
struct AppState(Arc<Inner>);
struct Inner {
    db: PathBuf,
    known_hosts: PathBuf,
    jobs: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

type Result<T> = std::result::Result<T, String>;

fn validate_request(request: &ScanRequest) -> Result<()> {
    if request.root.trim().is_empty() || request.root.contains('\0') {
        return Err("Choose a valid starting folder.".into());
    }
    match request.kind.as_str() {
        "local" | "share" => {
            if !Path::new(&request.root).is_absolute() {
                return Err("Choose an absolute folder path.".into());
            }
        }
        "ssh" => {
            if request.host.as_deref().unwrap_or("").trim().is_empty()
                || request.username.as_deref().unwrap_or("").trim().is_empty()
            {
                return Err("Enter an SSH host and username.".into());
            }
            if !matches!(request.platform.as_deref(), Some("windows" | "linux")) {
                return Err("Select the remote operating system.".into());
            }
        }
        _ => return Err("Unsupported scan source.".into()),
    }
    Ok(())
}

#[tauri::command]
fn default_root() -> String {
    std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| "C:\\".into())
}

#[tauri::command]
async fn start_scan(state: State<'_, AppState>, request: ScanRequest) -> Result<ScanInfo> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || start_scan_inner(&state, request))
        .await
        .map_err(|e| e.to_string())?
}
fn start_scan_inner(state: &AppState, request: ScanRequest) -> Result<ScanInfo> {
    validate_request(&request)?;
    let mut jobs = state
        .0
        .jobs
        .lock()
        .map_err(|_| "Scan coordinator is unavailable.")?;
    if !jobs.is_empty() {
        return Err("A scan is already running. Cancel it before starting another.".into());
    }
    let name = Path::new(request.root.trim_end_matches(['\\', '/']))
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(&request.root)
        .to_string();
    let info = ScanInfo {
        id: uuid::Uuid::new_v4().to_string(),
        source: request,
        status: "scanning".into(),
        started_at: chrono::Utc::now().to_rfc3339(),
        finished_at: None,
        root_name: name.clone(),
        entries: 0,
        files: 0,
        directories: 0,
        logical_bytes: "0".into(),
        allocated_bytes: None,
        issue_count: 0,
        current_path: String::new(),
        error: None,
    };
    let conn = store::connect(&state.0.db)?;
    store::save_scan(&conn, &info)?;
    store::save_node(
        &conn,
        &info.id,
        &NodeInfo {
            id: 0,
            parent_id: None,
            name,
            path: ".".into(),
            kind: "directory".into(),
            logical_bytes: "0".into(),
            allocated_bytes: None,
            files: 0,
            directories: 0,
            modified: None,
            complete: false,
            shared: false,
        },
    )?;
    let cancel = Arc::new(AtomicBool::new(false));
    jobs.insert(info.id.clone(), cancel.clone());
    let worker_state = state.clone();
    let worker_info = info.clone();
    std::thread::spawn(move || {
        let scan_id = worker_info.id.clone();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_worker(&worker_state, worker_info, cancel)
        }));
        if let Err(error) = result.unwrap_or_else(|_| {
            Err("The scanner stopped unexpectedly. Partial results have been kept.".into())
        }) {
            if let Ok(conn) = store::connect(&worker_state.0.db) {
                if let Ok(mut info) = store::get_scan(&conn, &scan_id) {
                    info.status = "failed".into();
                    info.finished_at = Some(chrono::Utc::now().to_rfc3339());
                    info.error = Some(error);
                    let _ = store::save_scan(&conn, &info);
                }
            }
        }
        if let Ok(mut jobs) = worker_state.0.jobs.lock() {
            jobs.remove(&scan_id);
        }
    });
    Ok(info)
}

fn run_worker(state: &AppState, mut info: ScanInfo, cancel: Arc<AtomicBool>) -> Result<()> {
    let conn = store::connect(&state.0.db)?;
    conn.execute_batch("BEGIN IMMEDIATE")
        .map_err(|e| e.to_string())?;
    let mut last_flush = Instant::now();
    let mut pending = 0usize;
    let mut finished = false;
    let mut root = store::node(&conn, &info.id, 0)?;
    let request = info.source.clone();
    let mut emit = |event: ScanEvent| -> Result<()> {
        match event {
            ScanEvent::Node { node } => {
                let n = NodeInfo::from(&node);
                if n.id == 0 {
                    root = n.clone();
                    info.logical_bytes = n.logical_bytes.clone();
                    info.allocated_bytes = n.allocated_bytes.clone();
                    info.files = n.files;
                    info.directories = n.directories;
                }
                store::save_node(&conn, &info.id, &n)?;
            }
            ScanEvent::Issue {
                path,
                kind,
                message,
            } => {
                info.issue_count += 1;
                store::save_issue(
                    &conn,
                    &info.id,
                    &IssueInfo {
                        path,
                        kind,
                        message,
                    },
                )?;
            }
            ScanEvent::Progress {
                entries,
                files,
                directories,
                logical_bytes,
                current_path,
            } => {
                info.entries = entries;
                info.current_path = current_path;
                info.files = files;
                info.directories = directories.saturating_sub(1);
                info.logical_bytes = logical_bytes.to_string();
            }
            ScanEvent::Finished { cancelled } => {
                finished = true;
                info.status = if cancelled { "cancelled" } else { "completed" }.into();
                info.finished_at = Some(chrono::Utc::now().to_rfc3339());
                info.current_path = String::new();
            }
        }
        pending += 1;
        if !finished && (pending >= 256 || last_flush.elapsed() >= Duration::from_millis(250)) {
            store::save_scan(&conn, &info)?;
            conn.execute_batch("COMMIT; BEGIN IMMEDIATE;")
                .map_err(|e| e.to_string())?;
            pending = 0;
            last_flush = Instant::now();
        }
        Ok(())
    };
    let result = if request.kind == "ssh" {
        let remote_request = remote::RemoteRequest {
            host: request.host.clone().unwrap_or_default(),
            port: request.port.unwrap_or(22),
            username: request.username.clone().unwrap_or_default(),
            key_path: request.key_path.clone().filter(|p| !p.is_empty()),
            root: request.root.clone(),
            platform: request.platform.clone().unwrap_or_else(|| "linux".into()),
        };
        remote::scan(&remote_request, &state.0.known_hosts, &cancel, &mut emit)
    } else {
        spacetrace_scanner::scan(Path::new(&request.root), &cancel, &mut emit)
    };
    drop(emit);
    if let Err(error) = result {
        info.status = if cancel.load(Ordering::Relaxed) {
            "cancelled"
        } else {
            "failed"
        }
        .into();
        info.error = if cancel.load(Ordering::Relaxed) {
            None
        } else {
            Some(error)
        };
        info.finished_at = Some(chrono::Utc::now().to_rfc3339());
    } else if !finished {
        info.status = if cancel.load(Ordering::Relaxed) {
            "cancelled"
        } else {
            "failed"
        }
        .into();
        info.finished_at = Some(chrono::Utc::now().to_rfc3339());
        if info.status == "failed" {
            info.error = Some(
                "The connection ended before the scan completed. Partial results have been kept."
                    .into(),
            );
        }
    }
    if info.status != "completed" {
        root = store::reconcile_partial(&conn, &info.id)?;
        info.logical_bytes = root.logical_bytes.clone();
        info.allocated_bytes = root.allocated_bytes.clone();
        info.files = root.files;
        info.directories = root.directories;
        info.entries = conn
            .query_row(
                "SELECT COUNT(*) FROM nodes WHERE scan_id=?1",
                [&info.id],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
    }
    store::save_scan(&conn, &info)?;
    conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
async fn list_scans(state: State<'_, AppState>) -> Result<Vec<ScanInfo>> {
    let db = state.0.db.clone();
    tauri::async_runtime::spawn_blocking(move || store::list_scans(&store::connect(&db)?))
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
async fn get_scan(state: State<'_, AppState>, scan_id: String) -> Result<ScanInfo> {
    let db = state.0.db.clone();
    tauri::async_runtime::spawn_blocking(move || store::get_scan(&store::connect(&db)?, &scan_id))
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
async fn browse(
    state: State<'_, AppState>,
    scan_id: String,
    node_id: i64,
    search: String,
    offset: u32,
    limit: u32,
    metric: String,
) -> Result<BrowseResult> {
    if search.len() > 1024 {
        return Err("Search is too long.".into());
    }
    let db = state.0.db.clone();
    tauri::async_runtime::spawn_blocking(move || {
        store::browse(
            &store::connect(&db)?,
            &scan_id,
            node_id,
            &search,
            offset,
            limit,
            &metric,
        )
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
async fn get_issues(
    state: State<'_, AppState>,
    scan_id: String,
    offset: u32,
    limit: u32,
) -> Result<Vec<IssueInfo>> {
    let db = state.0.db.clone();
    tauri::async_runtime::spawn_blocking(move || {
        store::issues(&store::connect(&db)?, &scan_id, offset, limit)
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
fn cancel_scan(state: State<'_, AppState>, scan_id: String) -> Result<()> {
    let jobs = state
        .0
        .jobs
        .lock()
        .map_err(|_| "Scan coordinator is unavailable.")?;
    if let Some(cancel) = jobs.get(&scan_id) {
        cancel.store(true, Ordering::Relaxed);
    }
    Ok(())
}
#[tauri::command]
async fn export_scan(state: State<'_, AppState>, scan_id: String, path: String) -> Result<()> {
    let db = state.0.db.clone();
    tauri::async_runtime::spawn_blocking(move || store::export(&db, &scan_id, Path::new(&path)))
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
async fn probe_host(
    state: State<'_, AppState>,
    host: String,
    port: u16,
) -> Result<remote::HostProbe> {
    let path = state.0.known_hosts.clone();
    tauri::async_runtime::spawn_blocking(move || remote::probe(&host, port, &path))
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
async fn trust_host(
    state: State<'_, AppState>,
    host: String,
    port: u16,
    key_line: String,
) -> Result<()> {
    let path = state.0.known_hosts.clone();
    tauri::async_runtime::spawn_blocking(move || remote::trust(&host, port, &key_line, &path))
        .await
        .map_err(|e| e.to_string())?
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let directory = std::env::var_os("SPACETRACE_DATA_DIR")
                .map(PathBuf::from)
                .unwrap_or(app.path().app_local_data_dir()?);
            let db = directory.join("scans.db");
            store::initialize(&db).map_err(std::io::Error::other)?;
            app.manage(AppState(Arc::new(Inner {
                db,
                known_hosts: directory.join("known_hosts"),
                jobs: Mutex::new(HashMap::new()),
            })));
            Ok(())
        })
        .on_window_event(|window, event| {
            if matches!(event, tauri::WindowEvent::CloseRequested { .. }) {
                let state = window.state::<AppState>();
                if let Ok(jobs) = state.0.jobs.lock() {
                    for cancel in jobs.values() {
                        cancel.store(true, Ordering::Relaxed);
                    }
                };
            }
        })
        .invoke_handler(tauri::generate_handler![
            default_root,
            start_scan,
            list_scans,
            get_scan,
            browse,
            get_issues,
            cancel_scan,
            export_scan,
            probe_host,
            trust_host
        ])
        .run(tauri::generate_context!())
        .expect("SpaceTrace could not start");
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    #[test]
    fn local_scan_persists_and_exports_real_totals() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("fixture");
        std::fs::create_dir_all(root.join("nested")).unwrap();
        std::fs::write(root.join("one.txt"), b"12345").unwrap();
        std::fs::write(root.join("nested/two.bin"), b"123456789012").unwrap();
        let db = temp.path().join("scans.db");
        store::initialize(&db).unwrap();
        let state = AppState(Arc::new(Inner {
            db: db.clone(),
            known_hosts: temp.path().join("known_hosts"),
            jobs: Mutex::new(HashMap::new()),
        }));
        let info = ScanInfo {
            id: "integration".into(),
            source: ScanRequest {
                kind: "local".into(),
                root: root.to_string_lossy().into(),
                host: None,
                port: None,
                username: None,
                key_path: None,
                platform: None,
            },
            status: "scanning".into(),
            started_at: chrono::Utc::now().to_rfc3339(),
            finished_at: None,
            root_name: "fixture".into(),
            entries: 0,
            files: 0,
            directories: 0,
            logical_bytes: "0".into(),
            allocated_bytes: None,
            issue_count: 0,
            current_path: String::new(),
            error: None,
        };
        let conn = store::connect(&db).unwrap();
        store::save_scan(&conn, &info).unwrap();
        store::save_node(
            &conn,
            &info.id,
            &NodeInfo {
                id: 0,
                parent_id: None,
                name: "fixture".into(),
                path: ".".into(),
                kind: "directory".into(),
                logical_bytes: "0".into(),
                allocated_bytes: None,
                files: 0,
                directories: 0,
                modified: None,
                complete: false,
                shared: false,
            },
        )
        .unwrap();
        run_worker(&state, info, Arc::new(AtomicBool::new(false))).unwrap();
        let result = store::get_scan(&conn, "integration").unwrap();
        assert_eq!(result.status, "completed");
        assert_eq!(result.logical_bytes, "17");
        assert_eq!(result.files, 2);
        let tree = store::browse(&conn, "integration", 0, "", 0, 20, "logical").unwrap();
        assert_eq!(tree.node.logical_bytes, "17");
        assert_eq!(tree.total_children, 2);
        assert_eq!(tree.node.directories, 1);
        assert_eq!(result.directories, 1);
        assert_eq!(tree.children[0].name, "nested");
        assert_eq!(tree.children[0].logical_bytes, "12");
        let report = temp.path().join("report.json");
        store::export(&db, "integration", &report).unwrap();
        let json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(report).unwrap()).unwrap();
        assert_eq!(json["nodes"].as_array().unwrap().len(), 4);
        assert_eq!(json["totals"]["logicalBytes"], "17");
    }
}

#[cfg(test)]
mod store_tests;
