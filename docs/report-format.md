# SpaceTrace scan report, version 1.1

The app's **Export report** writes UTF-8 JSON in one of two layouts, chosen by
the filename:

| Filename | Layout |
| --- | --- |
| `.json` | One JSON object with `nodes` and `issues` arrays. |
| `.ndjson` or `.jsonl` | One JSON record per line; see [NDJSON layout](#ndjson-layout). |
| either, followed by `.gz` | The same bytes, gzip-compressed (for example `report.ndjson.gz`). |

Both layouts carry the same fields. This is the public report format; the
scanner CLI's newline-delimited events are a separate internal protocol. Reports
can contain completed, running, cancelled, or failed scans.

For large scans, prefer NDJSON with gzip. A JSON document of a few million
entries is a single line of over a gigabyte, which many tools cannot open or
stream; NDJSON can be read line by line, and these reports usually compress
more than tenfold.

All byte measurements are nonnegative **decimal strings**, or `null` when
unavailable. Parse them with an integer type that preserves 64-bit precision
(for example, JavaScript `BigInt`), rather than `Number`. Zero is a measured value,
not a substitute for an unavailable measurement. IDs and counts are JSON integers.

## Report fields

| Field | Meaning |
| --- | --- |
| `schemaVersion` | Report contract version; currently `"1.1"`. |
| `application`, `applicationVersion` | `"SpaceTrace"` and the producing app's version. |
| `scanId` | Identifier of the saved scan. Repeated exports of that scan retain it. |
| `source` | `kind` (`local`, `share`, or `ssh`), selected `root`, and nullable `host`, `port`, and `platform`. SSH platforms are `windows` or `linux`; a null SSH port means the default, 22. |
| `scanStatus` | `scanning`, `completed`, `cancelled`, or `failed`. Completed means traversal finished; consult issues and node completeness for coverage. |
| `startedAt`, `finishedAt` | UTC timestamps. `finishedAt` is null while the scan is running. |
| `observationCutoff` | UTC time the export header was produced. It does not make the scan an atomic filesystem snapshot. |
| `isPartial` | True when status is not `completed`, or any issues were reported. Allocation and filename issues can make this true even if directory enumeration finished. |
| `measurementPolicy` | Human-readable descriptions under `logical`, `allocated`, `linkTraversal`, `cloudFiles`, `units`, `null`, and `consistency`. |
| `totals` | `logicalBytes`, nullable `allocatedBytes`, `knownAllocatedBytes`, and integer `unavailableAllocations`, `entries`, `files`, `directories`, and `issueCount`. Entries include the root and link/special nodes; directory count excludes the root. |
| `issueSummary` | Issue counts by kind, most frequent first: `[{"kind":"permissionDenied","count":510}, …]`. |
| `error` | Terminal scan error text, or null. Individual entry problems appear in `issues`. |
| `nodes` | Flat hierarchy, ordered by ascending node ID. Each node appears once with its latest saved state. |
| `issues` | Captured issues in discovery order, each containing `path`, `kind`, and `message`. Treat kind values as extensible strings. |

`allocatedBytes` is null as soon as any entry's allocation is unavailable, which
is common for whole-drive scans: a locked system file is enough. In that case
use `knownAllocatedBytes`, the sum over files, links, and special entries whose
own allocation is known. It is a lower bound: `unavailableAllocations` counts the
entries it leaves out, and the contents of skipped directories (see issues) are
not included. When `allocatedBytes` is available, the two values are equal for a
completed scan.

Source paths and hostnames are intentionally included. SSH usernames, passwords,
private keys, and private-key file references are omitted from the source object.
Reports should still be treated as potentially sensitive filesystem inventories.

## Nodes and the hierarchy

The root has `id: 0`, `parentId: null`, and `path: "."`. Other nodes refer to their
parent's ID. IDs are stable **within a scan**, not across separate scans. Parent
IDs precede child IDs, so a consumer can rebuild the tree while reading the array.

| Field | Type and meaning |
| --- | --- |
| `id`, `parentId` | Integer ID and integer/null parent ID. |
| `name`, `path` | Display name and root-relative path. Child paths use `/` separators, including Windows reports. |
| `kind` | `directory`, `file`, `link`, or `special`. |
| `logicalBytes` | Decimal string: regular file length, or observed descendant file lengths for a directory. |
| `allocatedBytes` | Decimal string or null: attributed file allocation, or observed descendant allocation for a directory. |
| `files` | A file has 1; a directory counts descendant regular-file paths; links/special entries have 0. |
| `directories` | Descendant directories, excluding this node. File/link/special nodes have 0. |
| `modified` | Source-reported UTC modification timestamp, or null. Timestamp precision varies by source. For a directory this is when its own entry list last changed. |
| `newestModified` | Directories only: the latest modification time of the directory or anything inside it, UTC with second precision. Absent on other kinds, when no timestamp was available, and in scans saved before report version 1.1. While a scan runs, open folders show the latest change observed so far. |
| `complete` | Whether enumeration of this node/subtree completed within the scan policy. Open, inaccessible, and interrupted branches remain false. |
| `shared` | Source-reported hard-link sharing flag. Detection support varies; false is not a guarantee of uniquely owned physical storage. |

Logical size counts each regular-file path, including hard-link aliases.
Allocation is attributed once to the first observed alias when reliable identity
is available; that attribution can move between folders in a later scan. Folder
entry storage, alternate data streams, shared-extent accounting, and filesystem
metadata are outside these file totals. Totals therefore need not equal a drive's
reported used capacity.

Links are not followed, and their reported file bytes are zero. Reparse, virtual,
or mounted directories may be excluded with issues according to the source's
scan policy. Windows SSH currently leaves allocation unavailable. A node can have
`complete: true` and `allocatedBytes: null`: enumeration and measurement
availability are separate facts.

An unreadable entry can appear only in `issues` if its metadata could not be read.
Do not infer that every issue has a corresponding node. Names that cannot be
represented as Unicode can have a lossy display path with an explicit issue;
use node IDs rather than display paths as unique keys.

## Example

This completed scan contains one five-byte file occupying one 4096-byte block.

```json
{
  "schemaVersion": "1.1",
  "application": "SpaceTrace",
  "applicationVersion": "0.3.0",
  "source": {
    "kind": "local",
    "root": "C:\\Example",
    "host": null,
    "port": null,
    "platform": null
  },
  "scanId": "5c10ac52-4583-4cee-a77a-916d4e4ef265",
  "scanStatus": "completed",
  "startedAt": "2026-09-21T12:00:00Z",
  "finishedAt": "2026-09-21T12:00:01Z",
  "observationCutoff": "2026-09-21T12:00:05Z",
  "isPartial": false,
  "measurementPolicy": {
    "logical": "Sum of file lengths per discovered path",
    "allocated": "Reported allocated bytes; hard links attributed to first observed path when identity available",
    "linkTraversal": "Do not follow links or junctions",
    "cloudFiles": "Metadata only; no content hydration",
    "units": "bytes as decimal strings",
    "null": "Measurement unavailable",
    "consistency": "Observation over time; not a filesystem snapshot"
  },
  "totals": {
    "logicalBytes": "5",
    "allocatedBytes": "4096",
    "knownAllocatedBytes": "4096",
    "unavailableAllocations": 0,
    "entries": 2,
    "files": 1,
    "directories": 0,
    "issueCount": 0
  },
  "issueSummary": [],
  "error": null,
  "nodes": [
    {
      "id": 0, "parentId": null, "name": "Example", "path": ".",
      "kind": "directory", "logicalBytes": "5", "allocatedBytes": "4096",
      "files": 1, "directories": 0, "modified": "2026-09-21T11:58:00Z",
      "complete": true, "shared": false,
      "newestModified": "2026-09-21T11:59:30Z"
    },
    {
      "id": 1, "parentId": 0, "name": "hello.txt", "path": "hello.txt",
      "kind": "file", "logicalBytes": "5", "allocatedBytes": "4096",
      "files": 1, "directories": 0, "modified": "2026-09-21T11:59:30Z",
      "complete": true, "shared": false
    }
  ],
  "issues": []
}
```

An issue has this shape:

```json
{"path":"restricted","kind":"permissionDenied","message":"Access denied."}
```

## NDJSON layout

Each line holds one JSON object whose `record` field says what it is. The same
example as NDJSON (header shortened here):

```text
{"record":"header","schemaVersion":"1.1","application":"SpaceTrace",…,"issueSummary":[],"error":null}
{"record":"node","id":0,"parentId":null,"name":"Example","path":".",…}
{"record":"node","id":1,"parentId":0,"name":"hello.txt","path":"hello.txt",…}
{"record":"end","nodes":2,"issues":0}
```

- The first line is the `header` record: every report field except `nodes` and
  `issues`.
- `node` records follow in ascending ID order, then `issue` records in discovery
  order. Their fields are identical to the JSON layout's array elements.
- The last line is the `end` record with the number of node and issue records.
  A file without it was truncated.
- Lines end with a line feed (LF). Treat unknown `record` values as extensions
  and skip them.

Reading a compressed NDJSON report line by line, for example in Python:

```python
import gzip, json
with gzip.open("report.ndjson.gz", "rt", encoding="utf-8") as lines:
    for line in lines:
        record = json.loads(line)
        if record["record"] == "node" and record["kind"] == "directory":
            print(record["path"], record["logicalBytes"], record.get("newestModified"))
```

Export reads one consistent SQLite snapshot and streams rows to a temporary file
before replacing the destination. It does not load the full hierarchy into memory.
During an active scan, the report contains the latest committed observations and
incomplete folder subtotals. Cancelled/interrupted results retain observed nodes;
unfinished branches remain incomplete. Reports are not currently importable
through the app. Consumers should check `schemaVersion` and tolerate additional
object fields in future compatible versions.

## Version history

- **1.1:** NDJSON layout and gzip compression; `totals.knownAllocatedBytes`,
  `totals.unavailableAllocations`, `issueSummary`, and directory
  `newestModified`. Readers of 1.0 JSON reports keep working if they ignore
  unknown fields.
- **1.0:** Initial JSON report.
