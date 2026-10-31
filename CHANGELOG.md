# Changelog

> **Language**: English | [简体中文](docs/zh_CN/CHANGELOG.md)

## [Unreleased] — 2026-10-31 (Round 2)

### Thread Creation & Error Propagation

- All worker threads in the `ff`/`wbfp` command modules and in the music player now use
  `thread::Builder::new().name(...).spawn(...)`; a spawn failure is propagated as an error (9 call sites in `ff`/
  `wbfp`).
  The panicking `std::thread::spawn` is banned by the AGENTS.md conventions because it panics when the OS cannot create
  a
  thread
- `DecoderRuntime::run()` and `PreEffectRuntime::run()` return `Result`; a failed spawn aborts `Player::new()` instead
  of
  logging and continuing
- Event-channel sink constructors (`PlayerEventChannelSink::new`, `DecoderEventChannelSink::new`,
  `PreEffectRuntimeEventChannelSink::new`) return `std::io::Result`; `Player::new()` / `Runtime::create()` propagate the
  error. The `new` constructors were removed from the sink traits; direct sinks construct infallibly
- `DecoderRuntime::stop()` still joins the decode thread when `send_skip` fails, then returns the first error

### Decoder Session Pairing

- The decoder's engine (format reader + decoder) and data (track metadata) are merged into a single `DecoderSession`
  stored in one `Arc<Mutex<Option<DecoderSession>>>`; track changes replace/take the session atomically under one lock
- Removed the old split `engine`/`data` `Option`s and the "data missing while engine present" branch
- Decode-loop error policy: real runtime failures (next_packet error, real ring-buffer write failure, failed EOF push,
  fatal non-frame-level decode error) emit a `DecoderEvent` error and stop the decode thread; a single-frame
  `DecodeError` is reported and skipped; `WriteRejected` during stop/track change/seek is normal control flow

### Logic Errors Use `unreachable!`

- `wb_files_pack` (`manager/file_ops.rs`, `manager/metadata.rs`, `pack_io/file_handle.rs`): all logic-error `panic!`
  branches were converted to `unreachable!`
- `metadata.rs`: `open_virtual_file`/`create_virtual_file` receiving a directory path is a runtime error and now returns
  `PackFileError` instead of aborting
- `ff`/`wbfp`: exhausting the per-worker progress-bar pool is an invariant violation and now aborts via `unreachable!`
- AGENTS.md adds two error-handling conventions: thread-creation failures MUST be returned, and logic errors MUST use
  `unreachable!` (runtime failures must never be turned into `unreachable!`)

### Music Player (`mp`)

- `-v/--volume` out of the 0.0–1.0 range is now an argument error returned before playback starts (previously panicked);
  the help text documents 0.0–1.0

## [Unreleased] — 2026-10-31

### Workspace Split

- Root `Cargo.toml` is now a cargo workspace (`resolver = "3"`, member `water_ball_tool_lib`); the root package keeps
  the `water_ball_tool` binary crate under `src/`
- Library code moved to `water_ball_tool_lib/src/`: `file_finder`, `wb_files_pack`, `tools`, `gakumasu` (debug-only)
- New library modules: `concurrent_ring` (concurrent initializable ring buffer) and `split_ring_buffer` (split
  lock/lock-free read-write ring buffer) — both used by the music player's decode/effect/audio pipelines
- Binary code now imports the library as `water_ball_tool_lib::*` (`command.rs`, `command/ff.rs`, `command/wbfp.rs`)

### Music Player (`mp`)

- New terminal music player subcommand `mp` (module `music_play`, enum variant `MP`, `src/command/music_play.rs`): plays
  audio files/directories, reads tags with `lofty`, builds a `PlayList`, and accepts stdin commands `next`/`n`,
  `Play`/`P`, `pause`/`p`, `exit`
- Arguments: positional `files...`, `-v/--volume` (0.0–1.0, default 0.75), `--verbose` (long only), `-m/--mode`
  (`sequential-loop`, `sequential`, `repeat-one`, `shuffle`, `periodic-shuffle`)
- Modules: `player`, `decoder_runtime`, `effect` (`resampler`, `cross_bridge`, `fade`), `play_list`, `runtime`,
  `event_dispatch`, `tools`; `tui/` is WIP and not wired into the command yet

### Audio Realtime Thread

- The cpal callback no longer takes locks, invokes event sinks, or writes logs; it only `try_send`s events and log
  records into a bounded channel (capacity 1024, dropped messages counted)
- The new `player-event-dispatch` thread (`command/music_play/event_dispatch.rs`) performs the locking, sink emission,
  and actual `tracing` output; stream errors are logged there too
- Track end is posted as a `TrackEnded` event instead of invoking sinks from the callback
- `Player::try_stop` now reclaims all threads in order: pause audio stream → stop pre-effect runtime (wakes producers
  blocked in `push_wait`) → stop decoder thread → stop event dispatch thread → join the `rx_cmd` thread (idempotent)

### Logging

- Replaced the old progress-bar `Write` adapter with `TerminalWriter`/`FileWriter` wrappers; `init_global_logging` now
  takes `(&mp, module)` and returns `Option<PathBuf>`, while `multi_progress_init(module)` creates the per-command
  `MultiProgress` and installs the subscriber
- Debug builds: terminal-only output at `debug` level. Release builds: terminal at `info` level + log file at `debug`
  level (`<ProjectDirs data_local_dir>/<crate>/logs/<module>/<module>-<timestamp>.log`); if the file cannot be created,
  logging falls back to terminal `debug` output

### File Finder Hashing (`ff`)

- New optional multi-value positional `[HASH]...` argument after `[OUT_PATH]`: `all`, `md5`, `sha256`, `sha512`,
  `sha1`, `blake3`, `blake2b`, `crc32` (aliases `*`, `m5`, `s256`, `s512`, `s1`, `b3`, `b2b`, `crc`)
- Per-file hashes are attached to `FileInfo` and included in the JSON output
- Hashing forces single-threaded file discovery; the hash work itself runs on `thread_count` worker threads
- Selecting MD5/SHA-1 logs an insecurity warning

### Code Standards

- `water_ball_tool_lib/src/lib.rs`: `#![cfg_attr(not(test), deny(clippy::expect_used))]` — `expect` is now denied in
  production builds (the test target keeps the warning)
- `command.rs`: new `progress_style(template)` helper — invalid templates log an error and fall back to the default
  style instead of `expect`-panicking

### Bug Fixes

- WBFP erase (`OverwriteStrategy::Random` / `Dod5220`): the write offset did not advance with each chunk, so data blocks
  larger than `DATA_DATA_BLOCK_LEN` only overwrote their first chunk; all passes now seek to `pos + write_len`
- `ff` hashing: a failed progress-channel send skipped `hash_type.reset()`, polluting the next file's hash; hashing now
  always finalizes/resets on every path
- Music player exit path completed: audio stream, pre-effect runtime, decoder thread, event dispatch thread, and the
  `rx_cmd` thread are all stopped/joined on `try_stop` instead of leaking until process exit
- Music player audio callback: event sink calls and logging moved to the dispatch thread (realtime jitter / priority
  inversion fix)

### Build

- `.cargo/native-config.toml` must be passed manually (`cargo --config .cargo/native-config.toml build --release`) to
  enable `target-cpu=native` and produce artifacts under `target/native/`
- New `debug-no-audio` feature: the audio callback skips ring-buffer reads, post effects, and volume scaling for fast
  playback-control debugging without audio

## [Unreleased] — 2026-07-30

### Doc Comment Normalization

- `cargo doc --no-deps` warnings reduced to zero (fixed 2 `SearchWarning` instances being parsed as unclosed HTML tags)
- `FileInfo::new()`, `PackVirtualFile::get_modified()`, `PackVirtualFile::set_modified()`: added missing public API doc
  comments
- `bytes_len_to_string()`, `path_to_string_vec()`, `path_remove_head()`: added algorithm notes and runnable doc-test
  examples
- `ManifestDataBlock::next_ver()` / `ver_is_older()` / `get_ver()` / `get_block_len()` / `get_block_len_us()`: added
  internal algorithm doc comments
- `bytes_len_to_string()` format string fixed `"{}"` → `"{:.2}"` to match the documented two-decimal output
- `path_remove_head()` fixed prefix comparison bug: `path_vec > head_vec` (lexicographic) →
  `path_vec[..head_vec.len()] == head_vec` (correct prefix matching)

### Code Cleanup & Format Normalization

- `src/wb_files_pack/*.rs`: globally unified `use` import ordering (std → third-party → crate-internal), merged
  scattered imports
- Reformatted multi-line expressions: `match` arms, method chains, tuple destructuring; eliminated line-too-long
  warnings
- Added `#[cfg(test)]` gating to test-only `assert!` / `debug_assert!` usage, removing unused warnings in release builds
- `src/gakumasu/data.rs`: fixed multi-line block comment alignment
- Fixed missing trailing newlines in several files (`simulator.rs`, `file_hash.rs`, `pack_io.rs`)

### WBFP Manager Refactoring

- `src/wb_files_pack/manager.rs`: `WBFPManagerRun.write_lock` refactored into standalone substruct
  `WBFPManagerRunLock { lock, path, file }` for better cohesion
- `WBFPManagerRunLock` implements `Clone` (`file` field set to `None` on clone to avoid duplicated file descriptors)
- `PackLockInfo.run_lock` type promoted from `bool` to `WBFPManagerRunLock`, carrying full lock state
- `src/wb_files_pack/data.rs`: `WBFilesPackManifest::file()` return type changed from `&Option<PackIO>` to
  `Option<&PackIO>`, more idiomatic Rust API
- `WBFPManager::save_all()` visibility adjusted from `pub(super)` to `pub(crate)`

### PackVirtualFile API Simplification

- `get_len()` / `get_modified()` removed the `Result` wrapper, returning `u64` / `u128` directly — internal lock
  acquisition no longer produces a recoverable error
- Lock poisoning recovery unified to `std::sync::PoisonError::into_inner`, replacing the closure `|e| e.into_inner()`
- `check_sync_compat()` uses `|` pattern to merge read-only/write-only match branches, eliminating redundant code

### Command-Layer Extraction

- `src/command/wbfp.rs`: unpack work loop extracted into standalone `wpfp_u_work()` function
- Hash-verify work loop extracted into `wpfp_h_work()` function
- Eliminated duplicate queue-consumption logic between unpack and hash-verify

### Conditional Compilation Optimization

- `gakumasu` module (WIP game simulator) gated behind `#[cfg(debug_assertions)]`; no longer compiled in release builds
- `net_server` module (WIP network server) likewise gated behind `#[cfg(debug_assertions)]`
- Library crate (`water_ball_tool_lib`) public API more lean in release

### Error Handling Unification

- `src/wb_files_pack/error.rs`: all `Display` implementations use inline format variables `{e}` `{msg}`, eliminating
  redundant `format!` calls
- Removed unnecessary `.into()` calls across the codebase (auto-converted via type inference)

### Misc

- `.vscode/launch.json`: simplified debug config names, added `--include-ignored` test-run config
- `src/tools.rs`: merged nested `use std::path` declarations
- `ManagerSync`: `access_mode` field initialization moved earlier to match declaration order
- `ManagerSync::create_new()`: added `path.try_exists()` pre-check for clearer existence semantics

### Warning Suppression Removal

- Removed all 5 lint-suppression attributes across the codebase: `#![allow(clippy::unwrap_used)]` in
  `src/file_finder/test.rs` and `src/wb_files_pack/pack_io/file.rs`, plus `#[allow(clippy::too_many_arguments)]` on 3
  functions in `src/file_finder.rs`
- Added `TestTool::expect_ok()` helper (`src/tools.rs`) for tests — a non-panicking unwrap replacement compatible with
  the crate-wide `#![deny(clippy::unwrap_used)]`
- Rewrote all 30 `unwrap()`/`expect()` calls in test code to `TestTool::expect_ok()` or an explicit `match` (thread
  `join()` errors are `Box<dyn Any + Send>`, not `Display`)
- AGENTS.md: added "Warning-suppression attributes are FORBIDDEN" rule — `#[allow(...)]`, `#![allow(...)]`,
  `#[expect(...)]` and any other lint-suppression attribute MUST NOT be added to production or test code
