use crate::models::*;
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
        }
        drop(rows);
        drop(stmt);
        directory.logical_bytes = logical.to_string();
        directory.allocated_bytes = allocated.map(|v| v.to_string());
        directory.files = files;
        directory.directories = directories;
        if id == 0 {
            directory.complete = false;
        }
        save_node(conn, scan_id, &directory)?;
    }
    node(conn, scan_id, 0)
}

pub fn export(db: &Path, scan_id: &str, destination: &Path) -> Result<()> {
    if !destination.is_absolute() {
        return Err("Choose a full destination path.".into());
    }
    if destination
        .extension()
        .and_then(|x| x.to_str())
        .map(|s| s.eq_ignore_ascii_case("json"))
        != Some(true)
    {
        return Err("Choose a filename ending in .json.".into());
    }
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
        let mut writer = BufWriter::new(temporary.as_file_mut());
        let source = serde_json::json!({"kind":scan.source.kind,"root":scan.source.root,"host":scan.source.host,"port":scan.source.port,"platform":scan.source.platform});
        let header = serde_json::json!({"schemaVersion":"1.0","application":"SpaceTrace","applicationVersion":env!("CARGO_PKG_VERSION"),"source":source,"scanId":scan.id,"scanStatus":scan.status,"startedAt":scan.started_at,"finishedAt":scan.finished_at,"observationCutoff":chrono::Utc::now().to_rfc3339(),"isPartial":scan.status!="completed" || scan.issue_count>0,"measurementPolicy":{"logical":"Sum of file lengths per discovered path","allocated":"Reported allocated bytes; hard links attributed to first observed path when identity available","linkTraversal":"Do not follow links or junctions","cloudFiles":"Metadata only; no content hydration","units":"bytes as decimal strings","null":"Measurement unavailable","consistency":"Observation over time; not a filesystem snapshot"},"totals":{"logicalBytes":scan.logical_bytes,"allocatedBytes":scan.allocated_bytes,"entries":scan.entries,"files":scan.files,"directories":scan.directories,"issueCount":scan.issue_count},"error":scan.error});
        let mut header_text = serde_json::to_string(&header).map_err(|e| e.to_string())?;
        header_text.pop();
        write!(writer, "{},\"nodes\":[", header_text).map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare("SELECT info FROM nodes WHERE scan_id=?1 ORDER BY id")
            .map_err(|e| e.to_string())?;
        let mut rows = stmt.query([scan_id]).map_err(|e| e.to_string())?;
        let mut first = true;
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let text: String = row.get(0).map_err(|e| e.to_string())?;
            if !first {
                writer.write_all(b",").map_err(|e| e.to_string())?;
            }
            first = false;
            writer
                .write_all(text.as_bytes())
                .map_err(|e| e.to_string())?;
        }
        writer
            .write_all(b"],\"issues\":[")
            .map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare("SELECT info FROM issues WHERE scan_id=?1 ORDER BY id")
            .map_err(|e| e.to_string())?;
        let mut rows = stmt.query([scan_id]).map_err(|e| e.to_string())?;
        first = true;
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let text: String = row.get(0).map_err(|e| e.to_string())?;
            if !first {
                writer.write_all(b",").map_err(|e| e.to_string())?;
            }
            first = false;
            writer
                .write_all(text.as_bytes())
                .map_err(|e| e.to_string())?;
        }
        writer.write_all(b"]}\n").map_err(|e| e.to_string())?;
        writer.flush().map_err(|e| e.to_string())?;
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
}
