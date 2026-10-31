# AGENTS.md — WaterBall Tool

> **Language**: English | [简体中文](docs/zh_CN/AGENTS.md)

Rust 2024 workspace CLI toolkit. Three commands: `ff` (parallel file finder with optional hashing), `wbfp` (custom
binary pack format), and `mp` (terminal music player).

## Commands

```bash
cargo build --release          # release build
cargo build --features debug-no-audio # debug build that skips audio processing (see Build options)
cargo --config .cargo/native-config.toml build --release # native-optimized build → target/native
cargo check                    # compile check only (fast)
cargo clippy                   # lint
cargo test                     # skip long-time tests (#[ignore] handled automatically)
cargo test -- --include-ignored # include long-time tests (full CI suite on main-branch pushes)
./target/release/water_ball_tool -h
```

## Build options

- The repository is a **workspace**: the root `Cargo.toml` defines the `water_ball_tool` binary package and lists
  `water_ball_tool_lib` as a member. `cargo build` from the repository root builds both.
- `.cargo/native-config.toml` is **not** applied automatically (cargo only auto-loads files named `config.toml`). Pass
  it explicitly: `cargo --config .cargo/native-config.toml build --release`. It compiles with
  `-C target-cpu=native` and redirects artifacts to `target/native/` (binary at
  `target/native/release/water_ball_tool`). Do not use a native build as a portable distribution.
- The `debug-no-audio` feature is a debugging aid for the `mp` command: the cpal callback skips ring-buffer reads,
  post-effect processing, and volume scaling, so playback control can be debugged quickly without real audio. It
  produces no usable audio.

## Testing conventions

- Slow tests are marked `#[ignore = "longtime"]`. Skip them with `cargo test` (skipped by default). Use
  `--include-ignored` for the full suite on main-branch CI pushes.
- Subcommand handlers initialize logging themselves through `crate::multi_progress_init(module)`; tests that call the
  handler (`ff::args`, `wbfp::commands`, `music_play::args`) therefore get a working `MultiProgress` without extra
  setup. Re-registering the subscriber is rejected by `try_init` and only logged.
- Test temp dirs live under `./temp/test/` (gitignored).
- Use `TestTool::prepare_test_dir(dir)` at test start and `TestTool::cleanup_test_dir(dir)` at test end from
  `water_ball_tool_lib/src/tools.rs` (both tolerate `NotFound`; cleanup panics on unexpected errors).
  `TestTool::remove_test_pack_files(path)` removes `.pack`/`.wbm`/`.lock` files after pack tests.
- Cross-platform tests use `#[cfg(unix)]` / `#[cfg(windows)]` for symlink creation and platform-specific paths.

### Test directory and resource lifecycle

- **Tests MUST NOT depend on each other**: each test is fully self-contained; the directories it consumes and produces
  are disjoint from every other test's directories.
- **Resource directory layout**:
    - Test fixtures — files a test *needs*: `./resources/test/`
    - Test outputs — files a test *produces*: `./temp/test/`
    - Sub-path pattern: `{module_path}/{ok|err}/{test_fn_name}/` — the module path MUST be complete and consistent,
      starting from the root module with sub-modules joined by `/` (matching the Rust module path, e.g.
      `wb_files_pack/manager_sync`). The module path and test function name MUST appear exactly as written in the code.
    - `ok/` holds tests expected to succeed; `err/` holds tests expected to fail. Classification follows the test's
      dominant intent: a test goes under `err/` if and only if its core assertion is an error assertion — `matches!` on
      an error variant, `catch_unwind` + `assert!(r.is_err())`, or `should_panic`. Judge by dominant intent, not by
      whether error steps occur inside the test. Mixed tests with comparable success+error weight MUST be split into one
      `ok/` and one `err/` test per the smallest-unit rule.
- **Test resources are read-only**:
    - A test MUST NOT modify files under `resources/test/`.
    - If a needed resource does not exist, generate it before any test body executes — preferably at compile time when
      building for tests, otherwise in a `#[cfg(test)]` setup stage that runs ahead of the test functions. The
      generation logic is test-only code, but it is never part of a test function body; when a test body starts, its
      resources must already exist and are read-only.
    - Generation that runs at test time MUST be wrapped in explicit comment delimiters:
      `// ===== TEST-ONLY RESOURCE GENERATION: START =====` ... `// ===== TEST-ONLY RESOURCE GENERATION: END =====`, and
      the delimiters MUST be placed inside the Rust test function that uses the resource — the caller of the generation
      logic — never inside the helper/generation function itself. If generation cannot be automated, provide a dedicated
      test-resource generation function with an explicit comment; generation MUST happen before the corresponding test
      code executes.
    - If a resource already exists, use it as-is; do NOT regenerate it on every test run.
    - If a resource's content needs to change, copy it into `temp/test/` first and modify the copy — never the resource
      itself.
- **Temp directory lifecycle**:
    - Before a test starts, ALWAYS attempt to delete its temp files; a missing path is NOT an error, but any other
      unexpected error must terminate the test as a failure. Then create the test's temp directory.
    - At test end, delete the temp files automatically ONLY when the test passed. If an unexpected error occurred during
      the test, keep the temp files as evidence (do not delete).
    - If the end-of-test cleanup itself hits an unexpected error (judged by code logic), the test FAILS.

### Test resource strategy

- **Two-layer resource strategy**:
    - Fixed, deterministic resources: live in `resources/test/`, used as-is and read-only. Reused across runs — a
      non-randomized test does NOT regenerate its resource files on every run; generate only when missing.
    - Dynamic, per-run artifacts: live in `temp/test/` under the standard `{module_path}/{ok|err}/{test_fn_name}/`
      layout, created and cleaned up per the temp directory lifecycle above.
- **Randomized tests**:
    - A randomized test MUST NOT generate resource files — it may only produce temp files under `temp/test/` (random
      fixtures are test artifacts, never resources).
    - On unexpected errors, temp files are kept (not deleted); the leftover files at the failure site ARE the
      reproduction — inspect them to diagnose. No seed-replay mechanism is required.
    - Randomized tests run ONLY in full-suite mode (`cargo test -- --include-ignored`), gated with
      `#[ignore = "longtime"]`.

### Test intent and error assertions

- **Scope tests to the smallest unit**: each test MUST cover exactly one behavior with minimal setup. Do not bundle
  unrelated assertions or multi-module flows into one test — if a test needs N independent behaviors, split it into N
  tests.
- **Assert expected outcomes, not "passing"**: the purpose of a test is verifying the concrete expected result. Pin down
  the exact expected value/state (byte content, length, empty error list, exact error variant). Broad assertions like
  `is_ok()` alone are insufficient whenever the expected value is knowable.
- **Fault-injection tests MUST constrain the error range**: when a test deliberately triggers an error, it MUST verify
  that exactly the intended error occurred and nothing else; any other error (wrong variant, wrong source, wrong
  payload) is a failure. Prefer type-level matching — error enum variant, `PoisonError`, `downcast_ref` payload — over
  coarse `is_err()`.
- **Never match on message text of externally-sourced errors**: OS/std/third-party error messages differ across
  platforms, locales and versions — do NOT assert on their `Display`/`to_string()` text. Match on error
  kind/type/variant instead. Exception: messages the test itself injects (e.g. `panic!("literal")` payloads) are fully
  controlled and MAY be matched exactly.

## Error handling conventions

- `water_ball_tool_lib/src/lib.rs` globally denies `clippy::unwrap_used`, and production (non-test) builds additionally
  deny `clippy::expect_used` (`#![cfg_attr(not(test), deny(clippy::expect_used))]` — the test target keeps the warning
  until existing test expectations are migrated). The binary crate `src/main.rs` also denies `clippy::unwrap_used`.
  Never suppress these lints. Never use `.unwrap()`, `.expect()`, or any panicking-unwrap in library or application
  code.
- **Recoverable errors MUST be propagated** via the `?` operator to the caller using the module's error type
  (`PackFileError` in `wb_files_pack`, `io::Error` or `SearchResult`/`SearchWarning` in `file_finder`).
- **Non-fatal errors (warnings)** that should not abort execution:
    - Synchronous API (`FileFinder::search`): collected in `SearchResult.warnings: Vec<SearchWarning>`.
    - Streaming API (`FileFinder::search_stream`): emitted as `SearchEvent::Warning(SearchWarning)` on the receiver
      channel.
- **Mutex poisoning**: use `.lock().unwrap_or_else(std::sync::PoisonError::into_inner)` to recover the lock — avoid
  panicking on poison.
- **Thread join**: use `handle.join().map_err(|_| ...)?` to propagate panics as errors rather than calling `.unwrap()`.
- **Thread creation failures MUST be returned**: create threads with `thread::Builder::new()...spawn(...)` and propagate
  the
  failure with `?` (mapping it into the module's error type when needed). `std::thread::spawn` panics when the OS cannot
  create the thread and MUST NOT be used. Never log a spawn failure and continue as if the thread existed.
- **Logic errors MUST use `unreachable!`**: code that can never be reached under correct logic is a logic error —
  reaching it
  means an invariant was violated. Such branches MUST abort via `unreachable!("<reason>")`; do not use `panic!` for
  them, and
  do not log-and-continue. Conversely, a failure that can genuinely happen at runtime (I/O error, poisoned lock, invalid
  user input, spawn failure) is NOT a logic error: it must be returned or reported, never converted into `unreachable!`.
- **Non-panicking helpers**: `crate::command::progress_style(template)` builds a `ProgressStyle`; an invalid template
  logs an error and returns the default style instead of panicking.
- **Warning-suppression attributes are FORBIDDEN**: `#[allow(...)]`, `#![allow(...)]`, `#[expect(...)]`, and any other
  lint-suppression attribute MUST NOT be added to production or test code. Do not silence a warning — fix the underlying
  cause instead (refactor the code, propagate the error, or use a non-panicking helper such as `TestTool::expect_ok` in
  tests).

## Architecture notes (non-obvious from file layout)

### Workspace and crate layout

- Root `Cargo.toml` is a **workspace** (`resolver = "3"`) with member `water_ball_tool_lib`; the same file declares the
  `water_ball_tool` binary package whose sources live in `src/`.
- `water_ball_tool_lib/src/lib.rs` exports the library modules: `file_finder`, `wb_files_pack`, `tools`,
  `concurrent_ring`, `split_ring_buffer`, and `gakumasu` (gated under `#[cfg(debug_assertions)]` — only compiled in
  debug builds).
- `src/main.rs` declares `mod command;` — `command.rs` is private to the binary crate and is **not** part of the
  library.
- Binary code references the library through crate-name-qualified paths — `water_ball_tool_lib::file_finder`,
  `water_ball_tool_lib::wb_files_pack`, `water_ball_tool_lib::tools`, `water_ball_tool_lib::concurrent_ring`,
  `water_ball_tool_lib::split_ring_buffer` — not `crate::`.
- `command/ff.rs` — `ff` subcommand handler: argument parsing, progress bar setup, file search orchestration, and
  optional hashing. Calls `FileFinder::search_stream()` from the library. Command-layer worker/hash threads are created
  with `thread::Builder` and spawn failures propagate as errors.
    - Optional multi-value **positional** `[HASH]...` argument after `[OUT_PATH]`: `all`, `md5`, `sha256`, `sha512`,
      `sha1`, `blake3`, `blake2b`, `crc32` (aliases `*`, `m5`, `s256`, `s512`, `s1`, `b3`, `b2b`, `crc`). Computed
      hashes are attached to each file entry and appear in the JSON output.
    - When hashing is enabled, file discovery is forced to a single thread (`ff_thread_count = 1`) while a separate hash
      worker pool of `thread_count` threads (resolved from `-t/--thread-count`, defaulting to available parallelism)
      consumes the shared ring buffer.
- `command/wbfp.rs` — `wbfp` subcommand handler: pack/unpack/hash-verify with multi-threaded file I/O, progress bars per
  worker thread, and streaming file discovery via `FileFinder::search_stream()`. All pack/unpack/verify workers are
  created with `thread::Builder`, and spawn failures propagate as errors.
- `command/music_play.rs` + `command/music_play/` — `mp` subcommand handler (module `music_play`, enum variant `MP`),
  terminal music player, see below.
- `command/test.rs` — integration tests for `ff` and `wbfp` commands, including cross-platform symlink cycle detection
  tests.
    - Longtime tests (`#[ignore = "longtime"]`) use `create_large_fixture()` to generate 1,000 random files (25 dirs ×
      40 files, random content/size via `rand` crate) for stress testing. These replace old system-dir-scraping tests.

### File finder API (`water_ball_tool_lib/src/file_finder.rs`)

- `FileFinder::search(path, skip_symlinks, pb_tx, max_threads)` — blocking multi-threaded search returning `FilesList`
  tree.
- `FileFinder::search_stream(path, skip_symlinks, pb_tx, max_threads)` — non-blocking streaming search; returns
  `(JoinHandle<io::Result<()>>, Receiver<(PathBuf, FileInfo)>)`. Workers pass `None` for results to skip HashMap
  collection overhead. Used by `wbfp` pack and `ff` hashing for the producer-consumer pattern.
- Symlink cycle detection uses per-thread inode chain: each worker maintains a `Vec<InodeKey>` of directories entered
  via symlinks. When encountering a symlink→dir, the target's inode is checked against the chain.
- Inode key: `(dev << 64) | ino` on Unix; `canonicalize()` on Windows.
- Rate-limited progress: 50 files or 10 dirs per update.
- Tree rebuild uses **iterative stack-based post-order traversal** (Pre/Post state machine) instead of recursion —
  avoids stack overflow on deeply nested directories. Parent lookup uses `rposition()` backward search for correct path
  matching.
- Test file: `water_ball_tool_lib/src/file_finder/test.rs` — covers symlink discovery, ancestor cycle detection,
  multi-level symlinks, skip flag, broken symlinks, mixed scenarios, chain propagation, and `search_stream`
  correctness.

### Ring buffer APIs

- `concurrent_ring` (`water_ball_tool_lib/src/concurrent_ring.rs`) — concurrent initializable ring buffer
  (`ConcurrentRing<T>`), used as the producer-consumer queue between search/decoder threads and workers.
- `split_ring_buffer` (`water_ball_tool_lib/src/split_ring_buffer.rs`) — split read/write ring buffer where each end can
  independently be locked or lock-free (`locked_ring_buffer`, `unlocked_read_locked_write_ring_buffer`, …); the audio
  callback uses the lock-free reader.

### Tools API (`water_ball_tool_lib/src/tools.rs`)

- `PathTool::path_to_string_vec(path)` — split path into string segments.
- `PathTool::path_remove_head(path, head)` — strip head prefix from path returning relative path.
- `bytes_len_to_string(len)` — format bytes as human-readable (B/KiB/MiB/GiB).
- `TestTool::prepare_test_dir(dir)` — remove any stale test temp dir (tolerates `NotFound`, panics on other errors) and
  recreate it. Called at test start.
- `TestTool::cleanup_test_dir(dir)` — remove the test temp dir at test end; only runs when the test passed (a failing
  test panics before reaching it, keeping evidence). Panics on cleanup errors, failing the test.
- `TestTool::remove_test_pack_files(path)` — cleanup helper for `.pack`/`.wbm`/`.lock` in tests.

### WBFP public API

- The public entry point is `ManagerSync` (`water_ball_tool_lib/src/wb_files_pack/manager_sync.rs`), not
  `WBFPManager`.
- `ManagerSync` wraps `WBFPManager` + `PackIO` behind `Arc<Mutex<>>` for thread-safe access.
- `ManagerSync::open(path)` — opens an existing pack file in read-only mode.
- `ManagerSync::create_new(path)` — creates a new pack file with default config (separate manifest, no COW). Fails if
  file exists.
- `ManagerSync::options() -> PackOpenOptions` — builder pattern for full control: `read()`, `write()`, `create()`,
  `create_new()`, `cow()`, `separate_manifest()`.
- `ManagerSync::open_virtual_file(path, end_pos)` — opens an existing virtual file in read-only mode.
- `ManagerSync::create_virtual_file(path)` — creates a new virtual file in write-only mode.
- `ManagerSync::virtual_file_options() -> VirtualFileOpenOptions` — builder for virtual file access: `read()`,
  `write()`, `create_new()`, `end_pos()`, `alloc_size()`. `alloc_size(Some(len))` pre-allocates a known file size at
  128B alignment; `None` (default) allocates unknown-size files as whole 4MiB blocks; ignored when opening an existing
  file.
- `PackVirtualFile` (renamed from `PackFileWR`) — virtual file handle implementing `Read`/`Write`/`Seek` with access
  mode enforcement.
    - `get_len()` returns `u64` (not `Result<u64>`); `get_modified()` returns `u128` (not `Result<u128>`).
- Delete/erase API: `delete_file`, `delete_dir_all`, `erase_file(strategy)`, `erase_dir_all(strategy)`.
    - Delete: removes metadata + structure, submits data blocks to GC (no overwrite).
    - Erase: overwrites data blocks + manifest blocks to storage, then GC + remove.
    - `OverwriteStrategy` enum: `Zero`, `Random` (rand crate), `Dod5220` (3-pass: 0x00→0xFF→random — each pass written
      to disk). Data blocks are overwritten chunk by chunk; the write offset advances by the chunk length in every
      strategy/pass.
    - Root protection: empty path list rejected. `child_locked_count` on `PackStruct` prevents deleting dirs with locked
      children.

### Music player (`mp`)

- `command/music_play.rs` — `mp` subcommand (module `music_play`, enum variant `MP`): collects tracks from
  files/directories (metadata read with `lofty`), builds a `PlayList`, and drives `Runtime` from a stdin command loop
  (`next`/`n`, `Play`/`P`, `pause`/`p`, `exit`).
- Arguments: positional `files...`; `-v/--volume <f32>` initial volume as a 0.0–1.0 fraction (default `0.75`;
  out-of-range values are rejected with an argument error before playback starts); `--verbose` (long only);
  `-m/--mode` with `sequential-loop` (default), `sequential`, `repeat-one`, `shuffle`, `periodic-shuffle`.
- Modules under `command/music_play/`:
    - `player.rs` — cpal output stream, transport commands (`PlayCmd`), volume/position atomics, `PlayerEvent` sinks,
      and `Player::try_stop()`.
    - `decoder_runtime.rs` — symphonia decode thread feeding the pre-effect ring buffer. The format reader/decoder and
      the
      track metadata live together in one `DecoderSession` behind a single `Arc<Mutex<Option<...>>>`, so they are
      swapped/taken as a pair. `run()` returns an error when the thread cannot be spawned; the decode loop reports real
      runtime errors and stops, skips frame-level `DecodeError`, and treats stop/track-change `WriteRejected` as normal
      flow.
    - `effect.rs` + `effect/runtime.rs` — effect traits and the locked pre-effect runtime thread; concrete effects are
      `effect/resampler.rs`, `effect/cross_bridge.rs`, `effect/fade.rs`.
    - `play_list.rs` — track list plus loop/shuffle logic (`sequential-loop`, `sequential`, `repeat-one`, `shuffle`,
      `periodic-shuffle`).
    - `runtime.rs` — playback orchestration: UI refresh thread, automatic track advance, event consumers.
    - `event_dispatch.rs` — dedicated event/log dispatch thread for the realtime audio callback.
    - `tools.rs` — `TimeMetrics` / `DurationAccumulator` profiling helpers.
    - `tui/` (`app.rs`, `ui.rs`, `channel_writer.rs`) — **WIP**, not wired into the command yet.
- **Realtime-thread rule**: the cpal callback MUST NOT lock, invoke sinks, or write logs. It only posts events and log
  records with non-blocking `try_send` into a bounded channel (capacity 1024; full channel drops and counts). The
  `player-event-dispatch` thread performs the locking, `PlayerEventSink::emit` calls, and real `tracing` output.
- Track end is detected through an atomic EOF flag; the callback posts a `TrackEnded` event instead of invoking sinks
  directly.
- **Exit order (`Player::try_stop`, idempotent)**: pause the audio stream → stop the pre-effect runtime (wakes producers
  blocked in `push_wait`) → stop the decoder thread → stop the event dispatch thread → join the `rx_cmd` thread.

### Logging + progress bars

- Each subcommand handler calls `crate::multi_progress_init(module)` (`module` = `"ff"`, `"wbfp"`, or
  `"music_play"`). It creates that command's `MultiProgress` and installs the global tracing subscriber via
  `init_global_logging(&mp, module) -> Option<PathBuf>`, which returns the created log-file path if any.
- The terminal layer wraps `MultiProgress` (`suspend()` around each line) so logs never tear the progress bars.
- Debug builds (`#[cfg(debug_assertions)]`): terminal-only output at `debug` level. Release builds: terminal at `info`
  level plus a log file at `debug` level with the bracketed format
  `[timestamp] [level] [thread] [file:line] message`.
- Log file path: `<ProjectDirs data_local_dir>/<crate>/logs/<module>/<module>-<YYYY-MM-DD_HH-MM-SS>.log`. If the file
  cannot be created, logging falls back to terminal-only `debug` output.
- `main.rs` itself only parses `Cli` and dispatches; progress/log initialization is per subcommand.

### Progress bar helpers (`command.rs`)

- `create_pb(mp)` — create and register an auto-ticking spinner in the `MultiProgress`; returns a `ProgressBar`.
- `progress_style(template) -> ProgressStyle` — build a style from a template string; an invalid template logs an
  error and falls back to the default bar style (no panic). Use this instead of `ProgressStyle::tem*().expect(...)`.
- Constants: `BUF_LEN` = 10 MiB (read/write buffer), `PACK_PROGRESS_STYLE_TEMPLATE` (pack/unpack/hash: prefix, bar,
  elapsed/ETA, percent, bytes), `SPINNER_TEMPLATE` (idle/suspended worker spinner).

### Code change markers

- `//AI===` / `//AI_END===` are code change-region markers.
- Every modified, added, or removed code segment MUST be wrapped in paired markers (`//AI===` start, `//AI_END===` end).
- The start marker may carry a one-line note describing the change.
- Markers wrap code only, never documentation: do NOT put a marker inside or around `///` / `//!` doc comments (a plain
  `//` line between doc lines silently drops the following doc text). A doc-comment-only change needs no marker at all;
  when code and its doc comment change together, mark only the code.

### gakumasu

- WIP game simulator under `water_ball_tool_lib/src/gakumasu/`. Not wired into CLI yet. Data types use Chinese naming.

## WBFP binary format quick reference

- File extension: `.pack` (pack file), `.wbm` (separate manifest), `.lock` (PID-based write lock).
- Uses A/B dual-block atomic writes for manifest data blocks, BLAKE3 for integrity.
- Progressive save: auto-saves every 64MiB written or 10,000 files added.
- Garbage collection: batch sort (`sort_unstable_by_key`) + single-pass merge of adjacent free blocks. O ((K+M) log
  (K+M)).
- Full spec: `docs/en_US/wb_files_pack/manifest-data.md`

## Documentation sync

- Every coding task MUST update all related documentation files to reflect changes made. This includes:
    - **Project root management/architecture docs**: `AGENTS.md`, `README.md`, and any other Markdown files at the
      repository root.
    - **Technical spec docs**: `docs/en_US/*.md` and `docs/zh_CN/*.md` — format specifications, architecture decisions,
      design documents.
    - **Code-level docs**: module-level doc comments (`//!`), public API doc comments (`///`), inline comments.
- Outdated documentation is treated as a defect. The implementation is not complete until all affected docs are updated.
- Sync scope includes but is not limited to: new public APIs, changed signatures, removed methods, new modules, changed
  behavior, and updated conventions.
- **Multilingual sync is mandatory**: every document has language versions — English (`docs/en_US/` and English-only
  root files) and Chinese (`docs/zh_CN/`). When any document is modified, ALL its language versions MUST be updated in
  the same change with identical content and structure; a document update is not complete until every language version
  reflects it.
- Every document MUST keep its language-switch links (`> **Language**: English | [简体中文](...)` /
  `> **语言**: [English](../...) | 简体中文`) pointing at the correct counterpart. When a document is added or removed,
  update all cross-references and language links to it in every language version.

## Specification file language

- Specification files (e.g. this file, `AGENTS.md`) MUST be written in English only — no mixed-language content. Code
  literals (identifiers, command names, path strings, error/panic messages) are exempt.
- The root specification file MUST stay English-only; its Chinese translation lives at `docs/zh_CN/AGENTS.md` as a
  reference copy kept in sync per the multilingual sync rule above.

## Bilingual comments

Comments are in Chinese + English. Chinese is primary; English is usually a translation, not independent documentation.
When adding comments, follow the existing bilingual pattern.
