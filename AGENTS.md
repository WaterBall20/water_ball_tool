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
- `command/ff.rs` — `ff` subcommand handler: argument parsing, progress bar setup, and file search orchestration. Calls `FileFinder::search()` from the library.
- `command/wbfp.rs` — `wbfp` subcommand handler: pack/unpack/hash-verify with multi-threaded file I/O, progress bars per worker thread, and streaming file discovery via `FileFinder::search_stream()`.
- `command/test.rs` — integration tests for `ff` and `wbfp` commands, including cross-platform symlink cycle detection tests.

### File finder API (`src/file_finder.rs`)
- `FileFinder::search(path, skip_symlinks, pb_tx, max_threads)` — blocking multi-threaded search returning `FilesList` tree.
- `FileFinder::search_stream(path, skip_symlinks, pb_tx, max_threads)` — non-blocking streaming search; returns `(JoinHandle<io::Result<()>>, Receiver<(PathBuf, FileInfo)>)`. Workers pass `None` for results to skip HashMap collection overhead. Used by `wbfp` pack command for producer-consumer pattern.
- Symlink cycle detection uses per-thread inode chain: each worker maintains a `Vec<InodeKey>` of directories entered via symlinks. When encountering a symlink→dir, the target's inode is checked against the chain.
- Inode key: `(dev << 64) | ino` on Unix; `canonicalize()` on Windows.
- Rate-limited progress: 50 files or 10 dirs per update.
- Test file: `src/file_finder/test.rs` — covers symlink discovery, ancestor cycle detection, multi-level symlinks, skip flag, broken symlinks, mixed scenarios, chain propagation, and `search_stream` correctness.

### Tools API (`src/tools.rs`)
- `PathTool::path_to_string_vec(path)` — split path into string segments.
- `PathTool::path_remove_head(path, head)` — strip head prefix from path returning relative path.
- `bytes_len_to_string(len)` — format bytes as human-readable (B/KiB/MiB/GiB).
- `TestTool::remove_test_pack_files(path)` — cleanup helper for `.pack`/`.wbm`/`.lock` in tests.

### WBFP public API
- The public entry point is `Allocator` (`src/wb_files_pack/allocator.rs`), not `WBFPManager`.
- `Allocator` wraps `WBFPManager` + `PackIO` behind `Arc<Mutex<>>` for thread-safe access.
- `Allocator::create_new_pack_file2(path)` uses defaults (separate manifest, no COW).
- `Allocator::create_new_pack_file(path, cow, separate_manifest)` for custom config.

### Logging + progress bars
- `main.rs` creates a `MultiProgress` and passes it to `init_global_logging()`, which routes `tracing` output through a `MultiProgressWriter` adapter so logs don't overwrite the progress bar.
- Debug mode: log level `debug`. Release mode: `info`. Controlled by `#[cfg(debug_assertions)]`.

### Progress bar helpers (`command.rs`)
- `create_pb(mp)` — create and register a spinner in the `MultiProgress`. Returns `None` in no-progress mode.
- `set_pb_style2(pb, data_len)` — switch to determinate bar mode (`=>-` chars), for known-total-length operations.
- `update_pb(cb, path1, path2, last_written, current_written)` — rate-limited progress callback (fires every 10×BUF_LEN bytes).
- Templates: `PROGRESS_STYLE_TEMPLATE` (search) and `PACK_PROGRESS_STYLE_TEMPLATE` (pack/unpack with prefix, percent, bytes).

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
