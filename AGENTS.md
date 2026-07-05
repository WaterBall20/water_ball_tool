# AGENTS.md — WaterBall Tool

Rust 2024 CLI toolkit. Two commands: `ff` (parallel file finder) and `wbfp` (custom binary pack format).

## Commands

```bash
cargo build --release          # release build
cargo check                    # compile check only (fast)
cargo clippy                   # lint
cargo test -- --skip longtime  # skip slow/integration tests (CI-compatible)
cargo test                     # all tests including slow
./target/release/water_ball_tool -h
```

## Testing conventions

- Slow tests are marked `#[ignore = "longtime"]`. Skip them with `cargo test -- --skip longtime`. CI also skips them (no `--include-ignored` flag).
- Tests that use `indicatif::MultiProgress` must call `crate::init_global_logging(&mp)` first — otherwise logging panics because tracing subscriber isn't initialized.
- Test temp dirs live under `./temp/test/` (gitignored).
- Use `TestTool::remove_test_pack_files(path)` from `tools.rs` to clean up `.pack`, `.wbm`, and `.lock` files after pack tests.
- Cross-platform tests use `#[cfg(unix)]` / `#[cfg(windows)]` for symlink creation and platform-specific paths.

## Architecture notes (non-obvious from file layout)

### Module graph
- `src/main.rs` declares `mod command;` — **not** `lib.rs`. `command.rs` is private to the binary.
- `src/lib.rs` exports the library crates: `file_finder`, `wb_files_pack`, `tools`, `gakumasu`.
- `command.rs` uses `water_ball_tool::file_finder` and `water_ball_tool::wb_files_pack` (crate-name-qualified paths), not `crate::` — because command is in the binary crate.

### WBFP public API
- The public entry point is `Allocator` (`src/wb_files_pack/allocator.rs`), not `WBFPManager`.
- `Allocator` wraps `WBFPManager` + `PackIO` behind `Arc<Mutex<>>` for thread-safe access.
- `Allocator::create_new_pack_file2(path)` uses defaults (separate manifest, no COW).
- `Allocator::create_new_pack_file(path, cow, separate_manifest)` for custom config.

### Logging + progress bars
- `main.rs` creates a `MultiProgress` and passes it to `init_global_logging()`, which routes `tracing` output through a `MultiProgressWriter` adapter so logs don't overwrite the progress bar.
- Debug mode: log level `debug`. Release mode: `info`. Controlled by `#[cfg(debug_assertions)]`.

### AI-generated code markers
- Some sections are delimited by `//AI===` / `//AI_END===`. These are AI-generated regions. Do not assume these delimiters are part of the project's style.

### gakumasu
- WIP game simulator under `src/gakumasu/`. Not wired into CLI yet. Data types use Chinese naming.

## WBFP binary format quick reference

- File extension: `.pack` (pack file), `.wbm` (separate manifest), `.lock` (PID-based write lock).
- Uses A/B dual-block atomic writes for manifest data blocks, BLAKE3 for integrity.
- Progressive save: auto-saves every 128KB written or 10,000 files added.
- Garbage collection: merges adjacent free blocks.
- Full spec: `docs/wb_files_pack/manifest-data.md`

## Bilingual comments

Comments are in Chinese + English. Chinese is primary; English is usually a translation, not independent documentation. When adding comments, follow the existing bilingual pattern.
