export type SourceKind = 'local' | 'share' | 'ssh';
export type Metric = 'logical' | 'allocated';
export interface ScanRequest { kind: SourceKind; root: string; host?: string; port?: number; username?: string; keyPath?: string; platform?: 'linux' | 'windows'; }
export interface ScanInfo { id: string; source: ScanRequest; status: 'scanning' | 'completed' | 'cancelled' | 'failed'; startedAt: string; finishedAt: string | null; rootName: string; entries: number; files: number; directories: number; logicalBytes: string; allocatedBytes: string | null; issueCount: number; currentPath: string; error: string | null; }
export interface NodeInfo { id: number; parentId: number | null; name: string; path: string; kind: string; logicalBytes: string; allocatedBytes: string | null; files: number; directories: number; modified: string | null; complete: boolean; shared: boolean; }
export interface IssueInfo { path: string; kind: string; message: string; }
export interface BrowseResult { node: NodeInfo; ancestors: NodeInfo[]; children: NodeInfo[]; totalChildren: number; }
export interface HostProbe { host: string; port: number; fingerprint: string; keyLine: string; trusted: boolean; }
