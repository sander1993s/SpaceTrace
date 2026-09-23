use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanRequest {
    pub kind: String,
    pub root: String,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub username: Option<String>,
    pub key_path: Option<String>,
    pub platform: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanInfo {
    pub id: String,
    pub source: ScanRequest,
    pub status: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub root_name: String,
    pub entries: u64,
    pub files: u64,
    pub directories: u64,
    pub logical_bytes: String,
    pub allocated_bytes: Option<String>,
    pub issue_count: u64,
    pub current_path: String,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeInfo {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub name: String,
    pub path: String,
    pub kind: String,
    pub logical_bytes: String,
    pub allocated_bytes: Option<String>,
    pub files: u64,
    pub directories: u64,
    pub modified: Option<String>,
    pub complete: bool,
    pub shared: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueInfo {
    pub path: String,
    pub kind: String,
    pub message: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowseResult {
    pub node: NodeInfo,
    pub ancestors: Vec<NodeInfo>,
    pub children: Vec<NodeInfo>,
    pub total_children: u64,
}

impl From<&spacetrace_scanner::Node> for NodeInfo {
    fn from(n: &spacetrace_scanner::Node) -> Self {
        Self {
            id: n.id,
            parent_id: n.parent_id,
            name: n.name.clone(),
            path: n.path.clone(),
            kind: n.kind.clone(),
            logical_bytes: n.logical_bytes.to_string(),
            allocated_bytes: n.allocated_bytes.map(|v| v.to_string()),
            files: n.files,
            directories: n.directories,
            modified: n.modified.clone(),
            complete: n.complete,
            shared: n.shared,
        }
    }
}
