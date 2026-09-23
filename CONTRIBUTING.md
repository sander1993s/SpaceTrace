# Contributing to SpaceTrace

SpaceTrace is maintained by [Smet Software Solutions](https://smetsoftwaresolutions.be). Bug reports, documentation improvements, and focused pull requests are welcome.

## Report a problem

Search existing issues before opening one. Include the SpaceTrace version, Windows version, source type (local, network share, or SSH), steps to reproduce, and the expected and actual behavior. For security vulnerabilities, follow [SECURITY.md](SECURITY.md) instead of opening a public issue.

Scan reports and screenshots can reveal folder names, file paths, usernames, hostnames, and network shares. Use a small synthetic fixture where possible. Remove personal or confidential data from reports, logs, screenshots, and examples before sharing them. Never submit credentials, private keys, or a real scan database.

## Set up development

Use Windows 11 x64 with:

- Node.js 22 or later and npm.
- Rust stable with the `x86_64-pc-windows-msvc` toolchain.
- Visual Studio C++ build tools and a Windows SDK.
- WebView2 and Windows OpenSSH Client (required by SSH verification tests).

From the repository root:

```powershell
npm ci
npm run desktop
```

See the [README](README.md) for architecture, scan behavior, and remote-device prerequisites.

## Validate changes

The complete Windows build, test, and installer workflow initializes the Visual Studio developer environment for you:

```powershell
.\scripts\build-windows.ps1
```

Use `-Jobs 2` to reduce Cargo concurrency on smaller machines. In an existing x64 Visual Studio developer shell, you can run the checks separately:

```powershell
npm run build
cargo test --workspace --locked
npm run package
```

Build the frontend before native workspace tests on a clean checkout. `npm run build` checks TypeScript and builds the interface; the Rust tests cover scanner fixtures, SSH validation, persistence, and report export. For scanner-only changes, `cargo test -p spacetrace-scanner --locked` is also useful.

For interface changes, exercise the affected flow in the desktop app, including keyboard navigation where relevant. Remote scanning changes may require a real SSH or SMB target; describe what you tested and any unavailable environment in the pull request. Use only devices and data you have permission to access.

Keep test data isolated. If setting `SPACETRACE_DATA_DIR`, point it at a disposable test-data directory outside the folder being scanned.

## Submit a pull request

- Keep changes small and focused; discuss substantial feature or architecture changes in an issue first.
- Describe the problem, resulting behavior, and checks performed. Add regression coverage for behavior changes where practical.
- Follow existing code style and update relevant documentation when behavior or report formats change.
- Commit lockfile changes when dependencies change. Include only source and intentional assets, excluding generated builds, installers, reports, and local databases.
- Check the licenses of new dependencies and copied assets, preserve required notices, and explain new dependencies in the pull request.

By submitting a contribution, you agree to make it available under the project's [MIT license](LICENSE). Contribute only material you have the right to share under those terms.
