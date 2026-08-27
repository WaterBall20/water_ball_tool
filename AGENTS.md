# AGENTS.md — WaterBall Tool

> **Language**: English | [简体中文](docs/zh_CN/AGENTS.md)

Rust 2024 CLI toolkit. Two commands: `ff` (parallel file finder) and `wbfp` (custom binary pack format).

## Commands

```bash
cargo build --release          # release build
cargo check                    # compile check only (fast)
cargo clippy                   # lint
cargo test                     # skip long-time tests (#[ignore] handled automatically)
cargo test -- --include-ignored # include long-time tests (full CI suite on main-branch pushes)
./target/release/water_ball_tool -h
```

## Testing conventions

- Slow tests are marked `#[ignore = "longtime"]`. Skip them with `cargo test` (skipped by default). Use `--include-ignored` for the full suite on main-branch CI pushes.
- Tests that use `indicatif::MultiProgress` must call `crate::init_global_logging(&mp)` first — otherwise logging panics because tracing subscriber isn't initialized.
- Test temp dirs live under `./temp/test/` (gitignored).
- Use `TestTool::prepare_test_dir(dir)` at test start and `TestTool::cleanup_test_dir(dir)` at test end from `tools.rs` (both tolerate `NotFound`; cleanup panics on unexpected errors). `TestTool::remove_test_pack_files(path)` removes `.pack`/`.wbm`/`.lock` files after pack tests.
- Cross-platform tests use `#[cfg(unix)]` / `#[cfg(windows)]` for symlink creation and platform-specific paths.

### Test directory and resource lifecycle

- **Tests MUST NOT depend on each other**: each test is fully self-contained; the directories it consumes and produces are disjoint from every other test's directories.
- **Resource directory layout**:
  - Test fixtures — files a test *needs*: `./resources/test/`
  - Test outputs — files a test *produces*: `./temp/test/`
  - Sub-path pattern: `{module_path}/{ok|err}/{test_fn_name}/` — the module path MUST be complete and consistent, starting from the root module with sub-modules joined by `/` (matching the Rust module path, e.g. `wb_files_pack/manager_sync`). The module path and test function name MUST appear exactly as written in the code.
  - `ok/` holds tests expected to succeed; `err/` holds tests expected to fail. Classification follows the test's dominant intent: a test goes under `err/` if and only if its core assertion is an error assertion — `matches!` on an error variant, `catch_unwind` + `assert!(r.is_err())`, or `should_panic`. Judge by dominant intent, not by whether error steps occur inside the test. Mixed tests with comparable success+error weight MUST be split into one `ok/` and one `err/` test per the smallest-unit rule.
- **Test resources are read-only**:
  - A test MUST NOT modify files under `resources/test/`.
  - If a needed resource does not exist, generate it before any test body executes — preferably at compile time when building for tests, otherwise in a `#[cfg(test)]` setup stage that runs ahead of the test functions. The generation logic is test-only code, but it is never part of a test function body; when a test body starts, its resources must already exist and are read-only.
  - Generation that runs at test time MUST be wrapped in explicit comment delimiters: `// ===== TEST-ONLY RESOURCE GENERATION: START =====` ... `// ===== TEST-ONLY RESOURCE GENERATION: END =====`, and the delimiters MUST be placed inside the Rust test function that uses the resource — the caller of the generation logic — never inside the helper/generation function itself. If generation cannot be automated, provide a dedicated test-resource generation function with an explicit comment; generation MUST happen before the corresponding test code executes.
  - If a resource already exists, use it as-is; do NOT regenerate it on every test run.
  - If a resource's content needs to change, copy it into `temp/test/` first and modify the copy — never the resource itself.
- **Temp directory lifecycle**:
  - Before a test starts, ALWAYS attempt to delete its temp files; a missing path is NOT an error, but any other unexpected error must terminate the test as a failure. Then create the test's temp directory.
  - At test end, delete the temp files automatically ONLY when the test passed. If an unexpected error occurred during the test, keep the temp files as evidence (do not delete).
  - If the end-of-test cleanup itself hits an unexpected error (judged by code logic), the test FAILS.

### Test resource strategy

- **Two-layer resource strategy**:
  - Fixed, deterministic resources: live in `resources/test/`, used as-is and read-only. Reused across runs — a non-randomized test does NOT regenerate its resource files on every run; generate only when missing.
  - Dynamic, per-run artifacts: live in `temp/test/` under the standard `{module_path}/{ok|err}/{test_fn_name}/` layout, created and cleaned up per the temp directory lifecycle above.
- **Randomized tests**:
  - A randomized test MUST NOT generate resource files — it may only produce temp files under `temp/test/` (random fixtures are test artifacts, never resources).
  - On unexpected errors, temp files are kept (not deleted); the leftover files at the failure site ARE the reproduction — inspect them to diagnose. No seed-replay mechanism is required.
  - Randomized tests run ONLY in full-suite mode (`cargo test -- --include-ignored`), gated with `#[ignore = "longtime"]`.

### Test intent and error assertions

- **Scope tests to the smallest unit**: each test MUST cover exactly one behavior with minimal setup. Do not bundle unrelated assertions or multi-module flows into one test — if a test needs N independent behaviors, split it into N tests.
- **Assert expected outcomes, not "passing"**: the purpose of a test is verifying the concrete expected result. Pin down the exact expected value/state (byte content, length, empty error list, exact error variant). Broad assertions like `is_ok()` alone are insufficient whenever the expected value is knowable.
- **Fault-injection tests MUST constrain the error range**: when a test deliberately triggers an error, it MUST verify that exactly the intended error occurred and nothing else; any other error (wrong variant, wrong source, wrong payload) is a failure. Prefer type-level matching — error enum variant, `PoisonError`, `downcast_ref` payload — over coarse `is_err()`.
- **Never match on message text of externally-sourced errors**: OS/std/third-party error messages differ across platforms, locales and versions — do NOT assert on their `Display`/`to_string()` text. Match on error kind/type/variant instead. Exception: messages the test itself injects (e.g. `panic!("literal")` payloads) are fully controlled and MAY be matched exactly.

## Error handling conventions

- `src/lib.rs` globally denies `clippy::unwrap_used`. Never suppress it. Never use `.unwrap()`, `.expect()`, or any panicking-unwrap in library or application code.
- **Recoverable errors MUST be propagated** via the `?` operator to the caller using the module's error type (`PackFileError` in `wb_files_pack`, `io::Error` or `SearchResult`/`SearchWarning` in `file_finder`).
- **Non-fatal errors (warnings)** that should not abort execution:
  - Synchronous API (`FileFinder::search`): collected in `SearchResult.warnings: Vec<SearchWarning>`.
  - Streaming API (`FileFinder::search_stream`): emitted as `SearchEvent::Warning(SearchWarning)` on the receiver channel.
- **Mutex poisoning**: use `.lock().unwrap_or_else(std::sync::PoisonError::into_inner)` to recover the lock — avoid panicking on poison.
- **Thread join**: use `handle.join().map_err(|_| ...)?` to propagate panics as errors rather than calling `.unwrap()`.
- **Warning-suppression attributes are FORBIDDEN**: `#[allow(...)]`, `#![allow(...)]`, `#[expect(...)]`, and any other lint-suppression attribute MUST NOT be added to production or test code. Do not silence a warning — fix the underlying cause instead (refactor the code, propagate the error, or use a non-panicking helper such as `TestTool::expect_ok` in tests).

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
- `TestTool::prepare_test_dir(dir)` — remove any stale test temp dir (tolerates `NotFound`, panics on other errors) and recreate it. Called at test start.
- `TestTool::cleanup_test_dir(dir)` — remove the test temp dir at test end; only runs when the test passed (a failing test panics before reaching it, keeping evidence). Panics on cleanup errors, failing the test.
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

### Code change markers

- `//AI===` / `//AI_END===` are code change-region markers.
- Every modified, added, or removed code segment MUST be wrapped in paired markers (`//AI===` start, `//AI_END===` end).
- The start marker may carry a one-line note describing the change.

### gakumasu
- WIP game simulator under `src/gakumasu/`. Not wired into CLI yet. Data types use Chinese naming.

## WBFP binary format quick reference

- File extension: `.pack` (pack file), `.wbm` (separate manifest), `.lock` (PID-based write lock).
- Uses A/B dual-block atomic writes for manifest data blocks, BLAKE3 for integrity.
- Progressive save: auto-saves every 64MiB written or 10,000 files added.
- Garbage collection: batch sort (`sort_unstable_by_key`) + single-pass merge of adjacent free blocks. O((K+M) log(K+M)).
- Full spec: `docs/en_US/wb_files_pack/manifest-data.md`

## Documentation sync

- Every coding task MUST update all related documentation files to reflect changes made. This includes:
  - **Project root management/architecture docs**: `AGENTS.md`, `README.md`, and any other Markdown files at the repository root.
  - **Technical spec docs**: `docs/en_US/*.md` and `docs/zh_CN/*.md` — format specifications, architecture decisions, design documents.
  - **Code-level docs**: module-level doc comments (`//!`), public API doc comments (`///`), inline comments.
- Outdated documentation is treated as a defect. The implementation is not complete until all affected docs are updated.
- Sync scope includes but is not limited to: new public APIs, changed signatures, removed methods, new modules, changed behavior, and updated conventions.
- **Multilingual sync is mandatory**: every document has language versions — English (`docs/en_US/` and English-only root files) and Chinese (`docs/zh_CN/`). When any document is modified, ALL its language versions MUST be updated in the same change with identical content and structure; a document update is not complete until every language version reflects it.
- Every document MUST keep its language-switch links (`> **Language**: English | [简体中文](...)` / `> **语言**: [English](../...) | 简体中文`) pointing at the correct counterpart. When a document is added or removed, update all cross-references and language links to it in every language version.

## Specification file language

- Specification files (e.g. this file, `AGENTS.md`) MUST be written in English only — no mixed-language content. Code literals (identifiers, command names, path strings, error/panic messages) are exempt.
- The root specification file MUST stay English-only; its Chinese translation lives at `docs/zh_CN/AGENTS.md` as a reference copy kept in sync per the multilingual sync rule above.

## Bilingual comments

Comments are in Chinese + English. Chinese is primary; English is usually a translation, not independent documentation. When adding comments, follow the existing bilingual pattern.
