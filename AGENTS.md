# AGENTS.md — WaterBall Tool

Rust 2024 CLI toolkit. Two commands: `ff` (parallel file finder) and `wbfp` (custom binary pack format).

## Commands

```bash
cargo build --release          # release build
cargo check                    # compile check only (fast)
cargo clippy                   # lint
cargo test                     # 跳过长时间测试（#[ignore] 自动处理）
cargo test -- --include-ignored # 包含长时间测试（主分支 CI 全量测试）
./target/release/water_ball_tool -h
```

## Testing conventions

- Slow tests are marked `#[ignore = "longtime"]`. Skip them with `cargo test` (默认跳过)。CI 主分支 push 时使用 `--include-ignored` 全量运行。
- Tests that use `indicatif::MultiProgress` must call `crate::init_global_logging(&mp)` first — otherwise logging panics because tracing subscriber isn't initialized.
- Test temp dirs live under `./temp/test/` (gitignored).
- Use `TestTool::remove_test_pack_files(path)` from `tools.rs` to clean up `.pack`, `.wbm`, and `.lock` files after pack tests.
- Cross-platform tests use `#[cfg(unix)]` / `#[cfg(windows)]` for symlink creation and platform-specific paths.

## Error handling conventions

- `src/lib.rs` globally denies `clippy::unwrap_used`. Never suppress it. Never use `.unwrap()`, `.expect()`, or any panicking-unwrap in library or application code.
- **Recoverable errors MUST be propagated** via the `?` operator to the caller using the module's error type (`PackFileError` in `wb_files_pack`, `io::Error` or `SearchResult`/`SearchWarning` in `file_finder`).
- **Non-fatal errors (warnings)** that should not abort execution:
  - Synchronous API (`FileFinder::search`): collected in `SearchResult.warnings: Vec<SearchWarning>`.
  - Streaming API (`FileFinder::search_stream`): emitted as `SearchEvent::Warning(SearchWarning)` on the receiver channel.
- **Mutex poisoning**: use `.lock().unwrap_or_else(std::sync::PoisonError::into_inner)` to recover the lock — avoid panicking on poison.
- **Thread join**: use `handle.join().map_err(|_| ...)?` to propagate panics as errors rather than calling `.unwrap()`.
- **Test code** for library modules (e.g. `file_finder::test`) may use `#![allow(clippy::unwrap_used)]` at the module level, but only where the `.unwrap()` is in a test setup/assertion context, never in production logic.

## Architecture notes (non-obvious from file layout)

### Module graph
- `src/main.rs` declares `mod command;` — **not** `lib.rs`. `command.rs` is private to the binary.
- `src/lib.rs` exports the library crates: `file_finder`, `wb_files_pack`, `tools`, `gakumasu` (gated under `#[cfg(debug_assertions)]` — only compiled in debug builds).
- `command.rs` uses `water_ball_tool::file_finder` and `water_ball_tool::wb_files_pack` (crate-name-qualified paths), not `crate::` — because command is in the binary crate.
- `command/ff.rs` — `ff` subcommand handler: argument parsing, progress bar setup, and file search orchestration. Calls `FileFinder::search()` from the library.
- `command/wbfp.rs` — `wbfp` subcommand handler: pack/unpack/hash-verify with multi-threaded file I/O, progress bars per worker thread, and streaming file discovery via `FileFinder::search_stream()`.
- `command/test.rs` — integration tests for `ff` and `wbfp` commands, including cross-platform symlink cycle detection tests.
  - Longtime tests (`#[ignore = "longtime"]`) use `create_large_fixture()` to generate 1,000 random files (25 dirs × 40 files, random content/size via `rand` crate) for stress testing. These replace old system-dir-scraping tests.

### File finder API (`src/file_finder.rs`)
- `FileFinder::search(path, skip_symlinks, pb_tx, max_threads)` — blocking multi-threaded search returning `FilesList` tree.
- `FileFinder::search_stream(path, skip_symlinks, pb_tx, max_threads)` — non-blocking streaming search; returns `(JoinHandle<io::Result<()>>, Receiver<(PathBuf, FileInfo)>)`. Workers pass `None` for results to skip HashMap collection overhead. Used by `wbfp` pack command for producer-consumer pattern.
- Symlink cycle detection uses per-thread inode chain: each worker maintains a `Vec<InodeKey>` of directories entered via symlinks. When encountering a symlink→dir, the target's inode is checked against the chain.
- Inode key: `(dev << 64) | ino` on Unix; `canonicalize()` on Windows.
- Rate-limited progress: 50 files or 10 dirs per update.
- Tree rebuild uses **iterative stack-based post-order traversal** (Pre/Post state machine) instead of recursion — avoids stack overflow on deeply nested directories. Parent lookup uses `rposition()` backward search for correct path matching.
- Test file: `src/file_finder/test.rs` — covers symlink discovery, ancestor cycle detection, multi-level symlinks, skip flag, broken symlinks, mixed scenarios, chain propagation, and `search_stream` correctness.

### Tools API (`src/tools.rs`)
- `PathTool::path_to_string_vec(path)` — split path into string segments.
- `PathTool::path_remove_head(path, head)` — strip head prefix from path returning relative path.
- `bytes_len_to_string(len)` — format bytes as human-readable (B/KiB/MiB/GiB).
- `TestTool::remove_test_pack_files(path)` — cleanup helper for `.pack`/`.wbm`/`.lock` in tests.

### WBFP public API
- The public entry point is `ManagerSync` (`src/wb_files_pack/manager_sync.rs`), not `WBFPManager`.
- `ManagerSync` wraps `WBFPManager` + `PackIO` behind `Arc<Mutex<>>` for thread-safe access.
- `ManagerSync::open(path)` — opens an existing pack file in read-only mode.
- `ManagerSync::create_new(path)` — creates a new pack file with default config (separate manifest, no COW). Fails if file exists.
- `ManagerSync::options() -> PackOpenOptions` — builder pattern for full control: `read()`, `write()`, `create()`, `create_new()`, `cow()`, `separate_manifest()`.
- `ManagerSync::open_virtual_file(path, end_pos)` — opens an existing virtual file in read-only mode.
- `ManagerSync::create_virtual_file(path)` — creates a new virtual file in write-only mode.
- `ManagerSync::virtual_file_options() -> VirtualFileOpenOptions` — builder for virtual file access: `read()`, `write()`, `create_new()`, `end_pos()`, `alloc_size()`. `alloc_size(Some(len))` pre-allocates a known file size at 128B alignment; `None` (default) allocates unknown-size files as whole 4MiB blocks; ignored when opening an existing file.
- `PackVirtualFile` (renamed from `PackFileWR`) — virtual file handle implementing `Read`/`Write`/`Seek` with access mode enforcement.
  - `get_len()` returns `u64` (not `Result<u64>`); `get_modified()` returns `u128` (not `Result<u128>`).
- Delete/erase API: `delete_file`, `delete_dir_all`, `erase_file(strategy)`, `erase_dir_all(strategy)`.
  - Delete: removes metadata + structure, submits data blocks to GC (no overwrite).
  - Erase: overwrites data blocks + manifest blocks to storage, then GC + remove.
  - `OverwriteStrategy` enum: `Zero`, `Random` (rand crate), `Dod5220` (3-pass: 0x00→0xFF→random — each pass written to disk).
  - Root protection: empty path list rejected. `child_locked_count` on `PackStruct` prevents deleting dirs with locked children.

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
- Progressive save: auto-saves every 64MiB written or 10,000 files added.
- Garbage collection: batch sort (`sort_unstable_by_key`) + single-pass merge of adjacent free blocks. O((K+M) log(K+M)).
- Full spec: `docs/wb_files_pack/manifest-data.md`

## Documentation sync

- Every coding task MUST update all related documentation files to reflect changes made. This includes:
  - **Project root management/architecture docs**: `AGENTS.md`, `README.md`, and any other Markdown files at the repository root.
  - **Technical spec docs**: `docs/*.md` — format specifications, architecture decisions, design documents.
  - **Code-level docs**: module-level doc comments (`//!`), public API doc comments (`///`), inline comments.
- Outdated documentation is treated as a defect. The implementation is not complete until all affected docs are updated.
- Sync scope includes but is not limited to: new public APIs, changed signatures, removed methods, new modules, changed behavior, and updated conventions.

## Bilingual comments

Comments are in Chinese + English. Chinese is primary; English is usually a translation, not independent documentation. When adding comments, follow the existing bilingual pattern.
