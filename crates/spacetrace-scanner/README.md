# SpaceTrace scanner

`scan(root, cancel, emit)` emits `ScanEvent` values synchronously. It only reads
filesystem metadata and enumerates directories; it never reads file contents,
changes permissions, deletes files, or follows symbolic links/junctions. The CLI
`spacetrace-scan <folder>` writes the same events as NDJSON, one event per line.

The root has ID `0` and relative path `.`. A node event is
`{"event":"node","node":{...}}`; node fields use camelCase. Issue, progress,
and finished events have their fields at the top level. Both event and node types
implement `Serialize` and `Deserialize`. Byte sizes are `u64` JSON numbers in this
internal protocol; public exports should convert sizes to decimal strings to
retain precision in JavaScript readers.

Directory nodes are upserts: the first record has empty totals and
`complete=false`, and the final record has all observed descendant totals. A
snapshot of open ancestors is also emitted about every 250 ms, so partially
scanned branches appear in the UI immediately. Snapshots are derived from cloned
counters and do not cause double counting when directories finish. A
directory's `directories` count excludes itself; its `files` count counts regular
file paths. File nodes have `files=1`, link/special nodes `files=0`. Progress counts
the root as one entry and one directory. Parent IDs are always smaller than child
IDs. Modified times are UTC RFC3339 with second precision, or null if unavailable
or before the Unix epoch.

Logical bytes count every regular file path, including hard-link aliases. Physical
allocation is deduplicated for known hard-link identities. The first observed
path owns allocation; traversal follows native directory order, not alphabetical
order, so allocation attribution can move between alias folders between scans.
`shared=true` marks multiply-linked files, including the first alias. Windows uses
metadata-only handles with `FILE_FLAG_OPEN_REPARSE_POINT`, standard allocation
information (compression information for compressed/sparse files), and file IDs.
Unix uses device/inode, link count, and `st_blocks * 512`. Directory entry storage
and symbolic-link storage are excluded from totals. Unsupported/unreadable
allocation is null and propagates to ancestors; it does not mark otherwise
complete logical enumeration incomplete. Alternate data streams are not scanned.

Reparse directories other than ordinary links/junctions are explicitly skipped
and reported, preventing cloud-only folders from being hydrated by traversal.
Selecting a reparse directory itself as the root is rejected. Regular cloud
files are inspected through metadata only. Inaccessible or disappearing entries
produce issues, and incomplete coverage propagates to all ancestors. Cancellation
closes directory iterators and emits final incomplete directory subtotals followed
by `{"event":"finished","cancelled":true}`. An event-sink error stops immediately.

Traversal retains only open ancestor directories plus identities of multiply-linked
files. It streams nodes and does not retain the entire scanned tree. A scan is a
live observation, not an atomic filesystem snapshot; files may change during it.
Names that cannot be represented as Unicode retain their real filesystem path
internally, have a lossy display/export path, and produce a `nonUnicodeName` issue.
IDs keep these nodes distinct even if display paths collide. Unix traversal stays
on the root filesystem; other mounted filesystems are recorded as incomplete
directories with an issue and can be scanned separately. Linux excludes `/proc`,
`/sys`, and `/dev`, including when selected as the root.

Run `cargo test -p spacetrace-scanner` from the workspace root. Symlink tests skip
only if the host denies symlink creation privileges.
