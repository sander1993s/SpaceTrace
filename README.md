# SpaceTrace

A Windows disk-space explorer by [Smet Software Solutions](https://smetsoftwaresolutions.be), released under the [MIT license](LICENSE). Find the folders consuming storage across this PC, accessible network shares, and Windows or Linux devices over SSH.

<img src="public/brand/spacetrace-mark.png" alt="SpaceTrace compass and storage map icon" width="128" height="128">

[Contributing](CONTRIBUTING.md) · [Security](SECURITY.md) · [Code signing policy (proposed)](CODE_SIGNING.md) · [Report a bug](https://github.com/sander1993s/SpaceTrace/issues)

## Run the app

Build from source using the instructions below. The Windows installer is written to `target/release/bundle/nsis/`. Install it for the current user, open SpaceTrace, choose a source and starting folder, and select **Start scan**. Local builds are unsigned and may trigger Windows publisher warnings. Code signing is a separate step for distributed installers.

- **This PC:** choose any readable local folder or drive. Recursive scanning stays inside ordinary directories and does not follow symbolic links or junctions.
- **Network share:** enter a UNC path such as `\\server\share` or choose an accessible mapped drive. Your existing Windows permissions apply.
- **SSH device:** enter host, port, username, operating system, and starting folder. Use your SSH agent or select a private-key file. Verify the presented SHA-256 host fingerprint before trusting a new host. Changed keys are refused.
- Explore storage through the interactive treemap, flat folder table, or expandable folder tree view. Switch between logical and allocated size, inspect scan issues, and export a `.json` report. Cancel preserves the results already discovered.

### Folder tree navigation

The folder tree view (`Tree view`) presents an expandable, logical top-to-bottom hierarchy starting from the scan root.

- **Expansion & drilldown:** click folder rows or chevrons to expand and collapse folders inline. Child branches load on demand in chunks of 200 items; select **Show more in `<folder>`** to paginate deeper directories without silent truncation.
- **File inspection:** click any file row to open its details panel with size measurements and copyable path.
- **Collapse all:** use the **Collapse all** button to quickly collapse all open branches while keeping the root visible.
- **Keyboard navigation:** standard tree keyboard accessibility (`role="tree"` / `role="treeitem"` with roving tab stop):
  - **Up / Down:** move focus between visible rows.
  - **Right Arrow:** expand a collapsed folder, or move to its first child if already expanded.
  - **Left Arrow:** collapse an expanded folder, or move to its parent folder.
  - **Home / End:** jump to the first or last visible row.
  - **Enter / Space:** toggle folder expansion or open file details.
  - **Tab:** move between the tree's roving tab stop and action buttons (pagination, retry, collapse all).

### SSH prerequisites

Windows OpenSSH Client must be installed on the computer running SpaceTrace. The target requires an enabled SSH server and key authentication. Linux targets need Python 3.7 or later; Windows targets need Windows PowerShell 5.1 or later. Encrypted keys should be loaded into an SSH agent. Password prompts, bastion/proxy connections, and interactive MFA are not supported in this release.

Bundled metadata helpers execute inline over the authenticated connection. They receive the chosen path as JSON, not shell command text. They do not install a daemon or leave a helper executable on the device. SSH scanning uses the selected account's permissions; it never elevates privileges. Long silence or transport loss ends the scan with a partial report.

## Understanding the numbers

- **Logical size:** file lengths summed over discovered paths. Two hard-linked names each contribute their apparent file length.
- **Allocated size:** filesystem-reported allocated file data. When reliable identity is available, multiple hard links are attributed once to the first path encountered. This is not a measurement of uniquely owned physical sectors; shared extents and snapshots complicate that distinction.
- **Unavailable:** represented by `null`, never zero. Windows SSH reports logical sizes and leaves allocation unavailable. Local Windows and Linux SSH report allocation where supported.
- **Partial:** inaccessible, excluded, changed, or interrupted content is disclosed in the issues panel. Folder subtotals cannot account for unreadable content, filesystem overhead, reserved space, or snapshots.

No file contents are read for classification. Cloud placeholders are treated conservatively and reparse directories are skipped to avoid hydration. Local scans and remote helpers do not follow links. Linux scans stay on the starting filesystem and skip virtual filesystem roots. The app makes no claim of a point-in-time filesystem snapshot.

Saved reports live in SpaceTrace's local application-data directory in SQLite. Source paths are private local data; no cloud account, telemetry, or upload service is used. JSON exports contain paths and scan metadata but no private keys, passwords, or private-key file references.

## Develop and build

Requirements: Windows 11 x64, Node.js 22 or later, Rust stable with the MSVC target, Visual Studio C++ build tools, and WebView2. The installer can bootstrap WebView2 on a clean machine.

```powershell
git clone https://github.com/sander1993s/SpaceTrace.git
cd SpaceTrace
npm ci
npm run desktop
```

Build and test from a normal PowerShell window:

```powershell
.\scripts\build-windows.ps1
```

The script locates Visual Studio through `vswhere`, initializes its x64 developer environment when needed, restores missing npm dependencies, builds the interface, runs the Rust tests, and packages the installer. Cargo uses four concurrent jobs by default; use `-Jobs 2` on a smaller machine. It requires the installed Windows OpenSSH Client for host-key tests and does not install system tools.

From an existing x64 Visual Studio developer shell, the equivalent commands are `npm run build`, `cargo test --workspace --locked`, and `npm run package`.

The native binary is `target/release/spacetrace.exe`. The per-user NSIS installer is in `target/release/bundle/nsis/`. To generate a managed-deployment MSI instead, use `npm run tauri -- build --bundles msi` with the required WiX prerequisites.

The independent scanner CLI is useful for fixtures and diagnostics:

```powershell
cargo run -p spacetrace-scanner --bin spacetrace-scan -- 'C:\Example'
```

It writes newline-delimited internal scan events. The app's **Export JSON** produces the versioned public report with exact decimal-string byte sizes; see [the report format](docs/report-format.md).

Set `SPACETRACE_DATA_DIR` only when using an isolated test database. The normal application uses its own local app-data folder. Never place a test database inside the directory being scanned.

## Code map

- `src/`: React interface, source wizard, folder tree, report navigation, and treemap layout.
- `crates/spacetrace-scanner/`: streaming metadata scanner and fixture tests.
- `src-tauri/src/`: native commands, SQLite persistence, export, and SSH transport.
- `src-tauri/helpers/`: inline Linux/Python and Windows/PowerShell SSH scanners.
- `assets/brand/`: original SpaceTrace logo and design notes.
- `public/brand/` and `src-tauri/icons/`: app and installer icon assets.

## Validation and current boundaries

The repository includes scanner fixtures, SSH protocol and validation checks, SQLite/export tests, and an integrated local scan-to-report test. Live SSH/SMB acceptance requires devices and credentials supplied by the operator; automated fixtures do not substitute for testing a real remote environment. Performance depends on filesystem, permissions, latency, and item count. No universal scan duration is promised.

GitHub Actions builds the interface, runs the Rust workspace tests, and builds the Windows installer for pushes and pull requests. See [the CI workflow](.github/workflows/ci.yml) and [contributor guidance](CONTRIBUTING.md) to run the same checks locally.
The app intentionally does not delete or move files, detect duplicate contents, schedule background scans, compare snapshots, or import reports yet. These features can follow after the scanning and reporting workflow has been used on real workloads.

## Brand

The SpaceTrace compass/lens contains a miniature storage map. Its Smet blue, teal, mint, ivory, and restrained gold connect it with the Smet identity and the illustrated Dolly Paste and MailHarbor product icons. See `assets/brand/BRAND.md` for official references and asset provenance.

## License

Copyright © 2026 [Smet Software Solutions](https://smetsoftwaresolutions.be). SpaceTrace is available under the [MIT license](LICENSE). Third-party dependencies retain their own licenses; see [third-party notices](THIRD_PARTY_NOTICES.md).
