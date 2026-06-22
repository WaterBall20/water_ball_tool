# AGENTS.md

## Build & Test

- **Requires Rust ≥ 1.85** (edition 2024)
- `cargo build` / `cargo build --release`
- `cargo check` — fast compile check without codegen
- `cargo clippy` — lint
- `cargo test -- --skip longtime` — skip slow tests (default local workflow)
- `cargo test --verbose` — **CI exact command** (runs everything including longtime tests)
- Long tests are marked `#[ignore = "longtime"]`
- No rustfmt.toml or clippy.toml — rely on defaults

## Code Conventions

- **Bilingual comments required**: every public function, struct, field, and module must have Chinese + English doc comments. Follow the existing pattern (Chinese first, then English with `///`).
- **AI markers**: `//AI===` / `//AI_END===` / `//AI==` mark AI-generated regions. These are intentional and must not be removed.
- Module tests live alongside the module as `module/test.rs` (not `tests/` directory). Integration-style tests for commands live in `src/command/test.rs`.
- Test temp files go under `./temp/test/<module>/`. `TestTool::remove_test_pack_files()` in `src/tools.rs` cleans up `.pack` + `.wbm` + `.lock`.
- Public API doc comments go on structs/functions in the library crate (`src/lib.rs`). They may repeat in `src/command.rs` for CLI-facing functions.

## Architecture

```
src/main.rs     — binary entry, CLI routing, tracing init via indicatif adapter
src/lib.rs       — library root, re-exports public modules
src/command.rs  — CLI command implementations (ff, wbfp)
src/file_finder.rs — multi-threaded directory scanner (FileFinder), outputs JSON
src/wb_files_pack/ — custom binary archive format (WBFP), the most complex subsystem
src/tools.rs     — small utilities (PathTool, bytes_len_to_string, TestTool)
src/gakumasu/    — game simulator (in development, incomplete)
```

## WBFP module (src/wb_files_pack/)

- **`Allocator`** (`allocator.rs`): public API entry point, thread-safe via `Arc<Mutex<WBFPManager>>` + `Arc<Mutex<PackIO>>`
- **`WBFPManager`** (`manager.rs`): internal manager (manifest, locks, file-level operations)
- **`PackIO`** (`pack_io.rs`): low-level file I/O, space allocation, GC
- **`data.rs`**: core data structures (Attribute, PackStruct, PackFileMetadata, ManifestDataBlock with A/B atomic write)
- **Tests**: `allocator/test.rs` tests public API; `manager/test.rs` tests internal API (bypasses Allocator). Both layers need updates for format changes.
- Binary format spec: `docs/wb_files_pack/manifest-data.md` — the authoritative reference for the `.pack`/`.wbm` binary layout
- Default constants: `separate_manifest=true` (`.wbm` separated), `cow=false`, hash type=1 (BLAKE3)

## CI

- GitHub Actions: `.github/workflows/rust_test.yml`
- Triggers: push/PR to `main`, manual dispatch
- Matrix: ubuntu-latest, windows-latest, macos-latest
- Uses `dtolnay/rust-toolchain@stable`

## Dependencies worth noting

- `indicatif` — progress bars; log output is routed through `MultiProgressWriter` adapter to coexist with progress bars
- `tracing` + `tracing-subscriber` — structured logging, `env-filter` feature, log level debug/release-aware
- `blake3` — file content hashing
- `pretty_assertions` — test assertions with diff output
