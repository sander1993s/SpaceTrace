# Third-party notices

SpaceTrace's own source and included SpaceTrace artwork are distributed under the [MIT license](LICENSE). Dependencies retain their upstream copyright notices and license terms; the project's MIT license does not replace them.

The application uses these main upstream projects:

- [Tauri](https://github.com/tauri-apps/tauri) and its [official plugins](https://github.com/tauri-apps/plugins-workspace) for the desktop shell.
- [React](https://github.com/facebook/react) and [Lucide](https://github.com/lucide-icons/lucide) for the interface.
- [Serde](https://github.com/serde-rs/serde), [serde_json](https://github.com/serde-rs/json), [rusqlite](https://github.com/rusqlite/rusqlite), [UUID](https://github.com/uuid-rs/uuid), [Chrono](https://github.com/chronotope/chrono), [base64](https://github.com/marshallpierce/rust-base64), [tempfile](https://github.com/Stebalien/tempfile), and [windows-rs](https://github.com/microsoft/windows-rs) for native functionality.
- [TypeScript](https://github.com/microsoft/TypeScript), [Vite](https://github.com/vitejs/vite), and their dependencies for development and builds.

Exact direct and transitive versions are recorded in `package-lock.json` and `Cargo.lock`. JavaScript dependency license metadata is recorded in `package-lock.json`; installed packages include their upstream license files. Rust dependency license metadata is available through `cargo metadata --locked --format-version 1`, and the downloaded crates contain their upstream license files. Preserve all applicable upstream notices when distributing compiled applications or dependency code.

Microsoft WebView2 and Windows OpenSSH are external runtime prerequisites with their own terms. Downloaded Smet, Dolly Paste, and MailHarbor design references are not included in this source distribution; their provenance is documented in [the brand notes](assets/brand/BRAND.md).
