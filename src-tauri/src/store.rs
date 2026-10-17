use crate::{models::*, recency};
use flate2::{write::GzEncoder, Compression};
use rusqlite::{params, Connection, OptionalExtension};
use std::{
    fs::{self},
    io::{BufWriter, Write},
    path::Path,
    time::Duration,
};
type Result<T> = std::result::Result<T, String>;

pub fn connect(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path).map_err(|e| e.to_string())?;
    conn.busy_timeout(Duration::from_secs(10))
        .map_err(|e| e.to_string())?;
    Ok(conn)
}

pub fn initialize(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let conn = connect(path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;
        CREATE TABLE IF NOT EXISTS scans(id TEXT PRIMARY KEY,started TEXT NOT NULL,info TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS nodes(scan_id TEXT NOT NULL,id INTEGER NOT NULL,parent_id INTEGER,name TEXT NOT NULL,path TEXT NOT NULL,kind TEXT NOT NULL,logical TEXT NOT NULL,allocated TEXT,info TEXT NOT NULL,PRIMARY KEY(scan_id,id));
        CREATE INDEX IF NOT EXISTS nodes_parent ON nodes(scan_id,parent_id);
        CREATE INDEX IF NOT EXISTS nodes_logical ON nodes(scan_id,parent_id,length(logical) DESC,logical DESC,name COLLATE NOCASE,id);
        CREATE INDEX IF NOT EXISTS nodes_allocated ON nodes(scan_id,parent_id,(allocated IS NULL),length(allocated) DESC,allocated DESC,name COLLATE NOCASE,id);
        CREATE TABLE IF NOT EXISTS issues(id INTEGER PRIMARY KEY,scan_id TEXT NOT NULL,info TEXT NOT NULL);
        CREATE INDEX IF NOT EXISTS issues_scan ON issues(scan_id,id);").map_err(|e|e.to_string())?;
    for mut scan in list_scans(&conn)? {
        if scan.status == "scanning" {
            conn.execute_batch("BEGIN IMMEDIATE")
                .map_err(|e| e.to_string())?;
            if conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM nodes WHERE scan_id=?1 AND id=0)",
                    [&scan.id],
                    |r| r.get::<_, bool>(0),
                )
                .map_err(|e| e.to_string())?
            {
                let root = reconcile_partial(&conn, &scan.id)?;
                scan.logical_bytes = root.logical_bytes;
                scan.allocated_bytes = root.allocated_bytes;
                scan.files = root.files;
                scan.directories = root.directories;
                scan.entries = conn
                    .query_row(
                        "SELECT COUNT(*) FROM nodes WHERE scan_id=?1",
                        [&scan.id],
                        |r| r.get(0),
                    )
                    .map_err(|e| e.to_string())?;
            }
            scan.status = "failed".into();
            scan.finished_at = Some(chrono::Utc::now().to_rfc3339());
            scan.error = Some("The previous scan was interrupted when SpaceTrace closed. Start a new scan to refresh it.".into());
            save_scan(&conn, &scan)?;
            conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

pub fn save_scan(conn: &Connection, scan: &ScanInfo) -> Result<()> {
    conn.execute("INSERT INTO scans(id,started,info) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET info=excluded.info", params![scan.id,scan.started_at,serde_json::to_string(scan).map_err(|e|e.to_string())?]).map_err(|e|e.to_string())?;
    Ok(())
}

pub fn get_scan(conn: &Connection, id: &str) -> Result<ScanInfo> {
    let data: Option<String> = conn
        .query_row("SELECT info FROM scans WHERE id=?1", [id], |r| r.get(0))
        .optional()
        .map_err(|e| e.to_string())?;
    serde_json::from_str(&data.ok_or("This scan could not be found.")?).map_err(|e| e.to_string())
}

pub fn list_scans(conn: &Connection) -> Result<Vec<ScanInfo>> {
    let mut stmt = conn
        .prepare("SELECT info FROM scans ORDER BY started DESC LIMIT 100")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?;
    rows.map(|r| serde_json::from_str(&r.map_err(|e| e.to_string())?).map_err(|e| e.to_string()))
        .collect()
}

pub fn save_node(conn: &Connection, scan_id: &str, n: &NodeInfo) -> Result<()> {
    conn.execute("INSERT INTO nodes(scan_id,id,parent_id,name,path,kind,logical,allocated,info) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9) ON CONFLICT(scan_id,id) DO UPDATE SET logical=excluded.logical,allocated=excluded.allocated,info=excluded.info",params![scan_id,n.id,n.parent_id,n.name,n.path,n.kind,n.logical_bytes,n.allocated_bytes,serde_json::to_string(n).map_err(|e|e.to_string())?]).map_err(|e|e.to_string())?;
    Ok(())
}

pub fn save_issue(conn: &Connection, scan_id: &str, issue: &IssueInfo) -> Result<()> {
    conn.execute(
        "INSERT INTO issues(scan_id,info) VALUES(?1,?2)",
        params![
            scan_id,
            serde_json::to_string(issue).map_err(|e| e.to_string())?
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn node(conn: &Connection, scan_id: &str, id: i64) -> Result<NodeInfo> {
    let data: Option<String> = conn
        .query_row(
            "SELECT info FROM nodes WHERE scan_id=?1 AND id=?2",
            params![scan_id, id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    serde_json::from_str(&data.ok_or("This folder is not available yet.")?)
        .map_err(|e| e.to_string())
}

pub fn browse(
    conn: &Connection,
    scan_id: &str,
    node_id: i64,
    search: &str,
    offset: u32,
    limit: u32,
    metric: &str,
) -> Result<BrowseResult> {
    let current = node(conn, scan_id, node_id)?;
    let mut ancestors = Vec::new();
    let mut parent = current.parent_id;
    while let Some(id) = parent {
        if ancestors.len() > 4096 {
            return Err("Invalid folder ancestry.".into());
        }
        let n = node(conn, scan_id, id)?;
        parent = n.parent_id;
        ancestors.push(n);
    }
    ancestors.reverse();
    let order = if metric == "allocated" {
        "allocated IS NULL ASC,length(allocated) DESC,allocated DESC,name COLLATE NOCASE,id"
    } else {
        "length(logical) DESC,logical DESC,name COLLATE NOCASE,id"
    };
    let escaped = search
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    let pattern = format!("%{}%", escaped);
    let searching = !search.trim().is_empty();
    let clause = if searching {
        "scan_id=?1 AND id != 0 AND name LIKE ?2 ESCAPE '\\'"
    } else {
        "scan_id=?1 AND parent_id=?2"
    };
    let param2 = if searching {
        rusqlite::types::Value::Text(pattern)
    } else {
        rusqlite::types::Value::Integer(node_id)
    };
    let total: u64 = conn
        .query_row(
            &format!("SELECT COUNT(*) FROM nodes WHERE {clause}"),
            params![scan_id, param2],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(&format!(
            "SELECT info FROM nodes WHERE {clause} ORDER BY {order} LIMIT ?3 OFFSET ?4"
        ))
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![scan_id, param2, limit.clamp(1, 500), offset], |r| {
            r.get::<_, String>(0)
        })
        .map_err(|e| e.to_string())?;
    let children = rows
        .map(|r| serde_json::from_str(&r.map_err(|e| e.to_string())?).map_err(|e| e.to_string()))
        .collect::<Result<Vec<NodeInfo>>>()?;
    Ok(BrowseResult {
        node: current,
        ancestors,
        children,
        total_children: total,
    })
}

pub fn issues(conn: &Connection, scan_id: &str, offset: u32, limit: u32) -> Result<Vec<IssueInfo>> {
    let mut stmt = conn
        .prepare("SELECT info FROM issues WHERE scan_id=?1 ORDER BY id LIMIT ?2 OFFSET ?3")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![scan_id, limit.clamp(1, 500), offset], |r| {
            r.get::<_, String>(0)
        })
        .map_err(|e| e.to_string())?;
    rows.map(|r| serde_json::from_str(&r.map_err(|e| e.to_string())?).map_err(|e| e.to_string()))
        .collect()
}

// Rebuild directory subtotals after an SSH interruption. Descending IDs visit
// children before parents, and rows stream from SQLite without a full tree in RAM.
pub fn reconcile_partial(conn: &Connection, scan_id: &str) -> Result<NodeInfo> {
    let mut cursor = i64::MAX;
    loop {
        let next:Option<(i64,String)>=conn.query_row("SELECT id,info FROM nodes WHERE scan_id=?1 AND kind='directory' AND id<?2 ORDER BY id DESC LIMIT 1",params![scan_id,cursor],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(|e|e.to_string())?;
        let Some((id, text)) = next else { break };
        cursor = id;
        let mut directory: NodeInfo = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        let mut logical = 0u64;
        let mut allocated = directory.allocated_bytes.as_ref().map(|_| 0u64);
        let mut files = 0u64;
        let mut directories = 0u64;
        let mut newest = directory
            .modified
            .as_deref()
            .and_then(recency::epoch_seconds);
        let mut stmt = conn
            .prepare("SELECT info FROM nodes WHERE scan_id=?1 AND parent_id=?2")
            .map_err(|e| e.to_string())?;
        let mut rows = stmt
            .query(params![scan_id, id])
            .map_err(|e| e.to_string())?;
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let text: String = row.get(0).map_err(|e| e.to_string())?;
            let n: NodeInfo = serde_json::from_str(&text).map_err(|e| e.to_string())?;
            logical = logical
                .checked_add(
                    n.logical_bytes
                        .parse::<u64>()
                        .map_err(|_| "Invalid byte total")?,
                )
                .ok_or("Byte total exceeds the supported range")?;
            allocated = match (allocated, n.allocated_bytes.as_ref()) {
                (Some(a), Some(b)) => {
                    a.checked_add(b.parse::<u64>().map_err(|_| "Invalid allocated total")?)
                }
                _ => None,
            };
            files = files.saturating_add(n.files);
            directories =
                directories.saturating_add(n.directories + u64::from(n.kind == "directory"));
            // Child directories were reconciled first (descending IDs).
            let child_newest = n
                .newest_modified
                .as_deref()
                .or(n.modified.as_deref())
                .and_then(recency::epoch_seconds);
            newest = newest.max(child_newest);
        }
        drop(rows);
        drop(stmt);
        directory.logical_bytes = logical.to_string();
        directory.allocated_bytes = allocated.map(|v| v.to_string());
        directory.files = files;
        directory.directories = directories;
        directory.newest_modified = newest.and_then(recency::format_seconds);
        if id == 0 {
            directory.complete = false;
        }
        save_node(conn, scan_id, &directory)?;
    }
    node(conn, scan_id, 0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReportLayout {
    /// One JSON document with `nodes` and `issues` arrays.
    Document,
    /// One JSON record per line: header, nodes, issues, then an end record.
    Lines,
}

/// The filename selects the layout; a trailing `.gz` adds gzip compression.
fn report_layout(destination: &Path) -> Result<(ReportLayout, bool)> {
    let name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_ascii_lowercase)
        .ok_or("Choose a valid filename.")?;
    let (stem, compressed) = match name.strip_suffix(".gz") {
        Some(stem) => (stem, true),
        None => (name.as_str(), false),
    };
    let layout = if stem.ends_with(".json") {
        ReportLayout::Document
    } else if stem.ends_with(".ndjson") || stem.ends_with(".jsonl") {
        ReportLayout::Lines
    } else {
        return Err(
            "Choose a filename ending in .json, .ndjson, or .jsonl, optionally followed by .gz."
                .into(),
        );
    };
    Ok((layout, compressed))
}

/// Saved node and issue rows are JSON objects; NDJSON records prepend a tag.
fn write_record(writer: &mut impl Write, record: &str, object: &str) -> Result<()> {
    let fields = object
        .strip_prefix('{')
        .ok_or("A saved scan record is damaged.")?;
    let separator = if fields.starts_with('}') { "" } else { "," };
    writeln!(writer, "{{\"record\":\"{record}\"{separator}{fields}").map_err(|e| e.to_string())
}

/// Lower bound of allocation: entries whose own size is known. Unlike the root
/// total, it stays available when a few files could not be measured.
fn allocation_summary(conn: &Connection, scan_id: &str) -> Result<(u128, u64)> {
    let mut stmt = conn
        .prepare("SELECT allocated FROM nodes WHERE scan_id=?1 AND kind<>'directory'")
        .map_err(|e| e.to_string())?;
    let mut rows = stmt.query([scan_id]).map_err(|e| e.to_string())?;
    let (mut known, mut unavailable) = (0u128, 0u64);
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        match row.get::<_, Option<String>>(0).map_err(|e| e.to_string())? {
            Some(bytes) => {
                known += u128::from(
                    bytes
                        .parse::<u64>()
                        .map_err(|_| "Invalid allocated total")?,
                )
            }
            None => unavailable += 1,
        }
    }
    Ok((known, unavailable))
}

fn issue_summary(conn: &Connection, scan_id: &str) -> Result<Vec<serde_json::Value>> {
    let mut stmt = conn
        .prepare(
            "SELECT COALESCE(json_extract(info,'$.kind'),'unknown') AS kind, COUNT(*) AS count
             FROM issues WHERE scan_id=?1 GROUP BY kind ORDER BY count DESC, kind",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([scan_id], |r| {
            Ok(serde_json::json!({"kind": r.get::<_, String>(0)?, "count": r.get::<_, u64>(1)?}))
        })
        .map_err(|e| e.to_string())?;
    rows.map(|r| r.map_err(|e| e.to_string())).collect()
}

fn write_report(
    writer: &mut impl Write,
    conn: &Connection,
    scan: &ScanInfo,
    layout: ReportLayout,
) -> Result<()> {
    let (known_allocated, unavailable_allocations) = allocation_summary(conn, &scan.id)?;
    let source = serde_json::json!({"kind":scan.source.kind,"root":scan.source.root,"host":scan.source.host,"port":scan.source.port,"platform":scan.source.platform});
    let header = serde_json::json!({"schemaVersion":"1.1","application":"SpaceTrace","applicationVersion":env!("CARGO_PKG_VERSION"),"source":source,"scanId":scan.id,"scanStatus":scan.status,"startedAt":scan.started_at,"finishedAt":scan.finished_at,"observationCutoff":chrono::Utc::now().to_rfc3339(),"isPartial":scan.status!="completed" || scan.issue_count>0,"measurementPolicy":{"logical":"Sum of file lengths per discovered path","allocated":"Reported allocated bytes; hard links attributed to first observed path when identity available","linkTraversal":"Do not follow links or junctions","cloudFiles":"Metadata only; no content hydration","units":"bytes as decimal strings","null":"Measurement unavailable","consistency":"Observation over time; not a filesystem snapshot"},"totals":{"logicalBytes":scan.logical_bytes,"allocatedBytes":scan.allocated_bytes,"knownAllocatedBytes":known_allocated.to_string(),"unavailableAllocations":unavailable_allocations,"entries":scan.entries,"files":scan.files,"directories":scan.directories,"issueCount":scan.issue_count},"issueSummary":issue_summary(conn, &scan.id)?,"error":scan.error});
    let header_text = serde_json::to_string(&header).map_err(|e| e.to_string())?;
    match layout {
        ReportLayout::Document => write!(
            writer,
            "{},\"nodes\":[",
            &header_text[..header_text.len() - 1]
        )
        .map_err(|e| e.to_string())?,
        ReportLayout::Lines => write_record(writer, "header", &header_text)?,
    }
    let nodes = write_rows(writer, conn, &scan.id, "nodes", "node", layout)?;
    if layout == ReportLayout::Document {
        writer
            .write_all(b"],\"issues\":[")
            .map_err(|e| e.to_string())?;
    }
    let issues = write_rows(writer, conn, &scan.id, "issues", "issue", layout)?;
    match layout {
        ReportLayout::Document => writer.write_all(b"]}\n"),
        // The end record lets readers detect a truncated file.
        ReportLayout::Lines => writeln!(
            writer,
            "{{\"record\":\"end\",\"nodes\":{nodes},\"issues\":{issues}}}"
        ),
    }
    .map_err(|e| e.to_string())
}

/// Streams saved rows in ID order without loading the hierarchy into memory.
fn write_rows(
    writer: &mut impl Write,
    conn: &Connection,
    scan_id: &str,
    table: &str,
    record: &str,
    layout: ReportLayout,
) -> Result<u64> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT info FROM {table} WHERE scan_id=?1 ORDER BY id"
        ))
        .map_err(|e| e.to_string())?;
    let mut rows = stmt.query([scan_id]).map_err(|e| e.to_string())?;
    let mut count = 0u64;
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        let text: String = row.get(0).map_err(|e| e.to_string())?;
        match layout {
            ReportLayout::Document => {
                if count > 0 {
                    writer.write_all(b",").map_err(|e| e.to_string())?;
                }
                writer
                    .write_all(text.as_bytes())
                    .map_err(|e| e.to_string())?;
            }
            ReportLayout::Lines => write_record(writer, record, &text)?,
        }
        count += 1;
    }
    Ok(count)
}

pub fn export(db: &Path, scan_id: &str, destination: &Path) -> Result<()> {
    if !destination.is_absolute() {
        return Err("Choose a full destination path.".into());
    }
    let (layout, compressed) = report_layout(destination)?;
    let conn = connect(db)?;
    conn.execute_batch("BEGIN DEFERRED")
        .map_err(|e| e.to_string())?;
    let scan = get_scan(&conn, scan_id)?;
    let parent = destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or("Choose a full destination path.")?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .map_err(|e| format!("Cannot create the export: {e}"))?;
    {
        let writer = BufWriter::new(temporary.as_file_mut());
        if compressed {
            let mut encoder = GzEncoder::new(writer, Compression::default());
            write_report(&mut encoder, &conn, &scan, layout)?;
            encoder
                .finish()
                .and_then(|mut writer| writer.flush())
                .map_err(|e| e.to_string())?;
        } else {
            let mut writer = writer;
            write_report(&mut writer, &conn, &scan, layout)?;
            writer.flush().map_err(|e| e.to_string())?;
        }
    }
    temporary.as_file().sync_all().map_err(|e| e.to_string())?;
    temporary.persist(destination).map_err(|e| {
        format!(
            "Could not finish the export; any existing report was preserved: {}",
            e.error
        )
    })?;
    conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
    Ok(())
}

/// Size of the database including its write-ahead log and shared-memory index.
fn database_bytes(db: &Path) -> u64 {
    ["", "-wal", "-shm"]
        .iter()
        .filter_map(|suffix| {
            let mut name = db.as_os_str().to_owned();
            name.push(suffix);
            fs::metadata(name).ok()
        })
        .map(|metadata| metadata.len())
        .sum()
}

/// Removes a saved scan's rows, then compacts the database so its space returns
/// to the drive. Only SpaceTrace's own records are touched, never scanned files.
/// The caller must ensure that no scan is running.
pub fn delete_scan(db: &Path, scan_id: &str) -> Result<DeleteOutcome> {
    let before = database_bytes(db);
    let conn = connect(db)?;
    conn.execute_batch("BEGIN IMMEDIATE")
        .map_err(|e| e.to_string())?;
    let removed = (|| {
        get_scan(&conn, scan_id)?;
        for statement in [
            "DELETE FROM nodes WHERE scan_id=?1",
            "DELETE FROM issues WHERE scan_id=?1",
            "DELETE FROM scans WHERE id=?1",
        ] {
            conn.execute(statement, [scan_id])
                .map_err(|e| e.to_string())?;
        }
        Ok::<_, String>(())
    })();
    if let Err(error) = removed {
        let _ = conn.execute_batch("ROLLBACK");
        return Err(error);
    }
    conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
    // VACUUM needs free space about the size of the database. If it fails, the
    // freed pages are still reused by later scans, so the deletion stands.
    let compacted = conn
        .execute_batch("VACUUM; PRAGMA wal_checkpoint(TRUNCATE);")
        .is_ok();
    drop(conn);
    Ok(DeleteOutcome {
        reclaimed_bytes: before.saturating_sub(database_bytes(db)).to_string(),
        compacted,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn export_is_valid_and_contains_no_key_reference() {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("test.db");
        initialize(&db).unwrap();
        let conn = connect(&db).unwrap();
        let info = ScanInfo {
            id: "fixture".into(),
            source: ScanRequest {
                kind: "local".into(),
                root: "fixture".into(),
                host: None,
                port: None,
                username: None,
                key_path: Some("SECRET_KEY_REFERENCE".into()),
                platform: None,
            },
            status: "cancelled".into(),
            started_at: "2026-01-01T00:00:00Z".into(),
            finished_at: None,
            root_name: "fixture".into(),
            entries: 1,
            files: 1,
            directories: 0,
            logical_bytes: "9007199254740993".into(),
            allocated_bytes: None,
            issue_count: 0,
            current_path: "".into(),
            error: None,
        };
        save_scan(&conn, &info).unwrap();
        let dest = temp.path().join("report.json");
        fs::write(&dest, "previous").unwrap();
        export(&db, "fixture", &dest).unwrap();
        let text = fs::read_to_string(dest).unwrap();
        assert!(!text.contains("SECRET_KEY_REFERENCE"));
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(json["totals"]["logicalBytes"], "9007199254740993");
        assert_eq!(json["isPartial"], true);
    }
    #[test]
    fn sorting_preserves_large_integer_order() {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("test.db");
        initialize(&db).unwrap();
        let conn = connect(&db).unwrap();
        let mut n = NodeInfo {
            id: 0,
            parent_id: None,
            name: "root".into(),
            path: ".".into(),
            kind: "directory".into(),
            logical_bytes: "0".into(),
            allocated_bytes: None,
            files: 0,
            directories: 0,
            modified: None,
            complete: true,
            shared: false,
            newest_modified: None,
        };
        save_node(&conn, "fixture", &n).unwrap();
        for (id, size) in [(1, "9"), (2, "10"), (3, "18446744073709551615")] {
            n.id = id;
            n.parent_id = Some(0);
            n.logical_bytes = size.into();
            n.name = format!("file{id}");
            save_node(&conn, "fixture", &n).unwrap();
        }
        let result = browse(&conn, "fixture", 0, "", 0, 20, "logical").unwrap();
        assert_eq!(
            result.children.iter().map(|n| n.id).collect::<Vec<_>>(),
            vec![3, 2, 1]
        );
    }

    /// A completed scan with one measured file, one unmeasured file and issues.
    fn report_fixture() -> (tempfile::TempDir, std::path::PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("test.db");
        initialize(&db).unwrap();
        let conn = connect(&db).unwrap();
        let info = ScanInfo {
            id: "report".into(),
            source: ScanRequest {
                kind: "local".into(),
                root: "C:\\Fixture".into(),
                host: None,
                port: None,
                username: None,
                key_path: None,
                platform: None,
            },
            status: "completed".into(),
            started_at: "2026-01-01T00:00:00Z".into(),
            finished_at: Some("2026-01-01T00:00:01Z".into()),
            root_name: "Fixture".into(),
            entries: 3,
            files: 2,
            directories: 0,
            logical_bytes: "15".into(),
            allocated_bytes: None,
            issue_count: 3,
            current_path: "".into(),
            error: None,
        };
        save_scan(&conn, &info).unwrap();
        let mut node = NodeInfo {
            id: 0,
            parent_id: None,
            name: "Fixture".into(),
            path: ".".into(),
            kind: "directory".into(),
            logical_bytes: "15".into(),
            allocated_bytes: None,
            files: 2,
            directories: 0,
            modified: None,
            complete: true,
            shared: false,
            newest_modified: Some("2025-05-05T05:05:05Z".into()),
        };
        save_node(&conn, "report", &node).unwrap();
        for (id, name, logical, allocated) in
            [(1, "a.txt", "5", Some("4096")), (2, "b.txt", "10", None)]
        {
            node.id = id;
            node.parent_id = Some(0);
            node.name = name.into();
            node.path = name.into();
            node.kind = "file".into();
            node.logical_bytes = logical.into();
            node.allocated_bytes = allocated.map(Into::into);
            node.files = 1;
            node.newest_modified = None;
            save_node(&conn, "report", &node).unwrap();
        }
        for kind in [
            "permissionDenied",
            "allocationUnavailable",
            "permissionDenied",
        ] {
            save_issue(
                &conn,
                "report",
                &IssueInfo {
                    path: "b.txt".into(),
                    kind: kind.into(),
                    message: "Access is denied.".into(),
                },
            )
            .unwrap();
        }
        (temp, db)
    }

    fn assert_summary(header: &serde_json::Value) {
        assert_eq!(header["schemaVersion"], "1.1");
        assert_eq!(header["totals"]["allocatedBytes"], serde_json::Value::Null);
        assert_eq!(header["totals"]["knownAllocatedBytes"], "4096");
        assert_eq!(header["totals"]["unavailableAllocations"], 1);
        assert_eq!(
            header["issueSummary"],
            serde_json::json!([{"kind":"permissionDenied","count":2},{"kind":"allocationUnavailable","count":1}])
        );
    }

    #[test]
    fn json_report_summarises_known_allocation_and_issue_kinds() {
        let (temp, db) = report_fixture();
        let dest = temp.path().join("report.json");
        export(&db, "report", &dest).unwrap();
        let json: serde_json::Value = serde_json::from_slice(&fs::read(dest).unwrap()).unwrap();
        assert_summary(&json);
        assert_eq!(json["nodes"].as_array().unwrap().len(), 3);
        assert_eq!(json["nodes"][0]["newestModified"], "2025-05-05T05:05:05Z");
        assert_eq!(json["issues"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn ndjson_report_has_one_tagged_record_per_line_and_an_end_record() {
        let (temp, db) = report_fixture();
        let dest = temp.path().join("report.ndjson");
        export(&db, "report", &dest).unwrap();
        let text = fs::read_to_string(dest).unwrap();
        let records: Vec<serde_json::Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let tags: Vec<&str> = records
            .iter()
            .map(|r| r["record"].as_str().unwrap())
            .collect();
        assert_eq!(
            tags,
            ["header", "node", "node", "node", "issue", "issue", "issue", "end"]
        );
        assert_summary(&records[0]);
        assert!(records[0].get("nodes").is_none());
        assert_eq!(records[1]["path"], ".");
        assert_eq!(records[3]["allocatedBytes"], serde_json::Value::Null);
        assert_eq!(records[4]["kind"], "permissionDenied");
        assert_eq!(
            records[7],
            serde_json::json!({"record":"end","nodes":3,"issues":3})
        );
        assert!(text.ends_with("}\n"));
    }

    #[test]
    fn gzip_reports_decompress_to_the_plain_layouts() {
        use std::io::Read;
        let (temp, db) = report_fixture();
        for (name, plain) in [
            ("report.json.gz", "plain.json"),
            ("REPORT.JSONL.GZ", "plain.jsonl"),
        ] {
            let dest = temp.path().join(name);
            export(&db, "report", &dest).unwrap();
            let mut text = String::new();
            flate2::read::GzDecoder::new(fs::File::open(&dest).unwrap())
                .read_to_string(&mut text)
                .unwrap();
            let plain_dest = temp.path().join(plain);
            export(&db, "report", &plain_dest).unwrap();
            let expected = fs::read_to_string(plain_dest).unwrap();
            // Only the export time differs between the two runs.
            let without_cutoff = |value: &str| {
                let start = value.find("\"observationCutoff\"").unwrap();
                let end = start + value[start..].find(',').unwrap();
                format!("{}{}", &value[..start], &value[end..])
            };
            assert_eq!(without_cutoff(&text), without_cutoff(&expected));
        }
    }

    #[test]
    fn report_filenames_select_layout_and_compression() {
        let path = |name: &str| std::path::PathBuf::from("C:\\reports").join(name);
        assert_eq!(
            report_layout(&path("a.json")).unwrap(),
            (ReportLayout::Document, false)
        );
        assert_eq!(
            report_layout(&path("a.JSON.gz")).unwrap(),
            (ReportLayout::Document, true)
        );
        assert_eq!(
            report_layout(&path("a.ndjson")).unwrap(),
            (ReportLayout::Lines, false)
        );
        assert_eq!(
            report_layout(&path("a.jsonl.gz")).unwrap(),
            (ReportLayout::Lines, true)
        );
        for rejected in ["a.gz", "a.txt", "a.json.zip", "json"] {
            assert!(report_layout(&path(rejected)).is_err(), "{rejected}");
        }
    }

    #[test]
    fn deleting_a_scan_keeps_other_scans_and_rejects_unknown_ids() {
        let (temp, db) = report_fixture();
        let conn = connect(&db).unwrap();
        let mut other = get_scan(&conn, "report").unwrap();
        other.id = "other".into();
        save_scan(&conn, &other).unwrap();
        save_node(&conn, "other", &node(&conn, "report", 1).unwrap()).unwrap();
        drop(conn);

        assert!(delete_scan(&db, "missing").is_err());
        let outcome = delete_scan(&db, "report").unwrap();
        assert!(outcome.compacted);
        let conn = connect(&db).unwrap();
        assert!(get_scan(&conn, "report").is_err());
        assert!(issues(&conn, "report", 0, 10).unwrap().is_empty());
        assert_eq!(list_scans(&conn).unwrap().len(), 1);
        assert_eq!(node(&conn, "other", 1).unwrap().name, "a.txt");
        drop(temp);
    }
}
