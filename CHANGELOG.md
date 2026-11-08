# Changelog

> **Language**: English | [简体中文](docs/zh_CN/CHANGELOG.md)

## [Unreleased] — 2026-11-01

### WBFP TUI

- `wbfp p`, `wbfp u` and `wbfp h` now run in a ratatui TUI when stdout is a terminal. The file list is read-only, shows
  per-file status and a global task card, and the right pane is reserved for a future content preview.
  `--no-tui` keeps the classic indicatif output; a non-TTY stdout falls back automatically.
- Added `wbfp browse <pack>` (alias `b`) for read-only browsing without starting a job. Directories load lazily through
  `ManagerSync`; pack mode browses the local source tree.
- `wbfp <pack>` without a subcommand now opens the file manager; the pack path is optional and this form is TUI-only:
  `--no-tui` and non-TTY input are rejected. The later entry below describes the manager layout.
- The TUI uses its own job thread and `JobEvent` channel; the UI opens a second read-only `ManagerSync` handle so manifest
  reads do not contend with pack/unpack/verify file handles. `q` during a job quits after it finishes.
- `wbfp.rs` was split into `classic.rs` (unchanged progress-bar path), `common.rs` (shared path/copy/extract),
  `events.rs`, `ops.rs` and `tui/` (state, rendering, lazy directory loading).
- The debug (log) and help popups are mutually exclusive and toggle closed with the same key that opened them (`F12`,
  `?`); `Esc` closes the open window. `d` no longer opens the debug popup.
- Directory rows aggregate descendant states, so a parent stays `⟳` until every child is finished and turns red/yellow if a
  child failed/was skipped. Hash verify's task card no longer shows an output field.
- `mp` and `wbfp` now share `command/tui_input.rs`: mouse capture, the Windows VT report parser, raw stdin reader,
  double-click state, bordered-list hit-testing and the wheel step. `wbfp` therefore supports mouse input on Windows
  instead of starting keyboard-only; if capture fails it continues keyboard-only. Under VT input both TUIs read raw
  stdin bytes, fixing the main view not receiving keys when crossterm 0.29 drops control characters. TUI diagnostics
  (route, keys, directory loads, job start/finish, per-GiB progress) are written to the debug log under
  `temp/logs/wbfp/`.
- Pack no longer emits directory events, so an empty source directory no longer shows a permanent in-progress icon.
- The preview pane now shows a live task list during jobs: active files first with per-file progress, completed files
  below. Worker threads enqueue rate-limited progress on a non-blocking channel; the UI keeps bounded queues and never
  reads files for the list, so task rendering cannot slow pack/unpack/verify I/O.
- File-list and task-list columns use fixed display widths (`unicode-width`), a fixed progress bar/percent/size column,
  and a fixed-height job summary, so fast small files no longer make the layout jump. Sizes now use separate right-aligned
  number and fixed-width unit columns, so the one-cell `B` unit cannot shift the number.
- The job card and per-file task bars now share `mp`'s eighth-block bar (`█▉▊▋▌▍▎▏`), resolving progress to 1/8 of a
  cell; the classic indicatif bars use the same character set.
- Write-optimization skips (same size and mtime) now report `Done` instead of a warning, and pack summaries no longer
  count them as warnings. The TUI also logs the write-optimization notice when opening an existing pack, matching the
  classic path. The job-card progress bar now keeps a full-width bar with its label on the next row, so the centered
  label no longer leaves a text-shaped hole in the bar.
- Without a subcommand, `wbfp` is now a file manager with local filesystem on the left, an optional pack in the middle,
  and preview on the right. The pack path is optional; bare `wbfp` hides the middle pane. The preview folds first, then
  the local pane; the pack pane stays. `Tab` switches panes, `Enter` opens a probed local pack, `Ctrl+W` closes it,
  `c`/`m` copy/move the selected entry to the other pane. `browse`, pack, unpack and verify keep their old layout.
- Local files are probed by the WBFilesPack header (not extension); valid packs show a magenta `◆`. The new
  `probe_pack` reads only the header and does not acquire `.lock`.

### WBFP Library

- Added `ManagerSync::rename(from, to)` for in-pack, same-directory leaf renames. It rewrites only the containing
  `PackStruct` block — data blocks, metadata positions and hashes are untouched — and rejects cross-directory moves;
  relocation will use a separate move API. The call returns `PackFileError::Lock` while the file or any descendant of
  a directory has an open virtual-file handle. Also fixed `child_locked_count` not counting the immediate parent
  directory when a directly contained file was opened.
- Added `ManagerSync::move_entry(from, to)` for cross-directory relocation with an optional leaf rename. The
  destination parent must already exist, same-directory calls are rejected (use `rename`), and missing/existing
  targets, moves into a directory's own subtree, open handles and read-only managers are rejected. It updates the
  cumulative counts on both ancestor chains and writes the destination `PackStruct` before the source one.

### Music Player TUI

- `mp` now runs a real TUI: the new `Runtime` owns the player and playlist, a background `mp-scan` thread turns the
  positional paths into tracks, and `tui::run` renders every 50 ms. Keys: `n` next, `←`/`→` seek 5 s per tap
  (hold previews, release commits), space play/pause, `p` pause,
  `P` play, `+`/`-` volume, `q`/`Esc`/`Ctrl+C` quit. Terminal setup/restore use `ratatui::try_init`/`try_restore`.
- The old `indicatif` UI thread, stdin command loop and `--ui-preview` flag are removed. Auto advance runs on the decode
  thread on `DecoderEventType::Eof` and switches immediately with `set_decoder_path_no_clear`: the old tail keeps
  playing from the buffers and the next track is queued behind it, so switching adds no delay and cuts nothing. Manual
  next clears the buffers. Track-open errors are classified as skip / retry / pause with a 5-failure cap.
- `Track` gained `album` and `audio_info` (filled by the scanner from lofty), and `PlayList::current_index()` exposes the
  current index for the playlist highlight.
- `-a/--autoplay` starts playback once the first track is found. Without it the first track is only loaded and stays
  paused; pressing play also loads the first track when none is loaded yet.
- Startup initialization is mode-aware: sequential modes load the first track immediately; `shuffle` and
  `periodic-shuffle` wait for 5 tracks and fall back to initializing with the available tracks when the scan ends early.
  Manual `n`/play before that point cancels the automatic initialization.
- `F12` toggles a two-column debug popup with the pre-effect/audio buffer occupancy over capacity, the path and format
  currently being decoded, the playback (device) format, playlist state, `lock_fail`/`underrun` counters and the
  `TimeMetrics` windows for decode, effect and callback
  stages on the left; the live log is on the right. `DecoderRuntime::decoder_path()` was added to expose the decoding
  path.
- The decoder batches interleaved samples in its internal buffer up to the pre-effect chain's `min_input_len_work()`
  hint
  before pushing a block; the hint is advisory and a smaller input may still produce output depending on the effect.
- The debug popup now shows buffer-based latency estimates: decoder-internal buffer, decode→pre-effect queue, pre-effect
  delay lines, playback ring plus the last callback block, and their total. The estimate is drawn as a proportional
  pipeline bar from decode (left) to playback (right). Device/OS latency is not included yet.
- Fixed the latency estimate accumulating across track changes: clearing the pre-effect input ring (skip/seek/restart)
  now
  also resets the queued-sample gauge, since the discarded blocks are never popped by the effect thread.
- Added an effects window (`e` or `:effects`): a two-column view with pre- and post-effects on the left (name and
  enabled state only) and the selected effect's chain, delay, description and settings placeholder on the right.
  `↑`/`↓` and `PageUp`/`PageDown` move the selection. Rows come only from `PreEffect`/`PostEffect` trait methods
  (`display_name`, `description`, `enabled`, `buffered_samples`), so new effects appear without UI changes. `Space`/
  `Enter`
  toggles the selected effect's enabled state through the same trait, and other settings editing is not implemented yet.
- Effect toggles are now applied safely. Pre-effect toggles go through a dedicated command channel and are applied by
  the
  effect thread; `ConcurrentRing::wake_blocked_readers` wakes a blocked `pop_wait` with the new `ReadWoken` error
  without consuming data. Disabling a pre effect drains it via `PreEffectDiscontinuity::DisableSelf` and passes the
  drained samples through downstream `UpstreamDisabled { distance }` handlers; downstream is not reset unless the
  effective format changes. The resampler is statically non-disableable (`DisablePolicy::Forbidden`) because bypassing
  it would change the device-format output. Post effects are strictly synchronous and toggle directly. `reset` now takes
  a `ResetReason` so an effect can distinguish a track start from a format change or toggle. The UI no longer updates
  optimistically; disabled effects report zero latency and the detail pane marks forbidden effects as fixed.
- Split-ring `write_all` now returns a dedicated `WriteAllError { written, kind }` so every failure reports how many
  elements were written. `wake_blocked_writers` is non-destructive (`Interrupted`; the caller can resume from
  `written`),
  while `reject_pending_writers` keeps clear-and-reject (`Rejected`). The pre-effect thread caches the unwritten
  remainder,
  applies the queued toggle, and resumes the cache before new audio, so a control wake cannot truncate a block.
- Fixed the effects window staying empty: the pre-effect thread now drops the effect-chain lock before writing to the
  output ring, so a paused/full ring cannot block `effect_summaries()` indefinitely. The chain lock is still held for
  `process`, and the UI keeps the last successful snapshot.
- The footer is narrower: `←`/`→` seek hints moved to the progress gauge, `↑`/`↓` scroll hints to the playlist title,
  and volume moved to a compact percentage gauge on the right of the progress row. The now-playing title no longer
  carries the volume. The effects button is kept in the footer as `e 效果`; debug uses `F12`.
- The main progress bar now uses eighth blocks (`█▉▊▋▌▍▎▏`), so the position moves in 1/8-cell steps instead of whole
  cells; the time/percent label sits to the right of the bar so the bar stays continuous. The shared helper also drives
  the `wbfp` job card and task bars.
- Logs moved from the main page into the right pane of the `F12` debug window; the main page's right side is now a
  reserved panel. Debug, effects and help windows are mutually exclusive, and `Esc` closes the active window first.
- TUI state was grouped into `playback`, `playlist`, `scan`, `seek`, `footer`, `mouse`, `command`, `effects` and
  `debug`; a single `Window` enum replaces the window booleans. Keys route exclusively by active window: command input
  first, `Esc` closes it, `Window::None` goes to the main view, and open windows never fall through to main keys.
- The playlist panel now renders only the visible rows and supports scrolling: `↑`/`↓` scroll one line,
  `PageUp`/`PageDown` one page, `Home`/`End` jump to the ends, the mouse wheel scrolls three lines and a left
  double-click
  on a row plays that track (a single click is ignored to avoid misfires). The viewport follows the current track on
  track changes. `PlayList::jump_to` and
  `Runtime::play_at` were added for index jumps, and the TUI enables mouse capture while it runs.
- Fixed mouse movement quitting the TUI on Windows/ConPTY. The terminal can deliver mouse reports as bare X10 payload
  bytes, so a coordinate byte such as `q` was handled as the quit key. Windows now enables
  `ENABLE_VIRTUAL_TERMINAL_INPUT`, requests SGR mouse reporting and parses the VT stream with `vtparse` (`VtInput`),
  mapping wheel and left-click events to the playlist; motion/middle/right/release are ignored. If VT input cannot be
  enabled, the mouse is disabled and the keyboard remains usable.
- TUI logging: `channel_writer` installs the tracing subscriber and routes formatted lines into the panel through a
  bounded channel (capacity 1024, `try_send` drops when full); debug builds show `debug` in the panel and also write a
  `debug` log file under the project temp directory (`temp/logs/<module>/`), so a crashed TUI still leaves evidence;
  release builds show `info` there and keep writing the `debug` log file to the user data directory. Panics and TUI
  loop errors are logged to that file, and key/mouse events are logged at `debug`. `main.rs` exposes its file writer
  and log filter to that layer, and one `write` call now splits every complete line instead of losing everything after
  the first newline.
- Log levels are colorized in the panel: the timestamp is dimmed and the level token is TRACE dark gray, DEBUG blue,
  INFO green, WARN yellow, ERROR red. The level is parsed from the first 40 bytes of the formatted line, so a message
  that happens to contain a level word cannot change the color.
- The `mp` TUI gained `:` command mode: the footer becomes an input line (`Enter` runs, `Esc` cancels, `Backspace`
  edits). Commands match by unique prefix: `quit`, `play`, `pause`, `toggle`, `next`, `prev`, `seek <mm:ss|+N|-N>`,
  `vol <0-100|+N|-N>`, `mode <name>`, `goto <n>`, `debug` and `help`/`?`; unknown or ambiguous names are reported in
  the footer. `help` opens a popup listing the command table. `Runtime::prev_track`, `Runtime::seek` and
  `Runtime::set_loop_mode` were added.
- Implemented `Player::try_seek` (now `&self`): clear the output ring, run the decoder's accurate seek, invalidate the
  EOF state and rebase the progress counter. `DecoderRuntime::try_seek` clears the pre-effect input ring and sends
  `AudioData::Seek` instead of a full effect reset; the resampler and cross-bridge effects drop their buffered state on
  `Seek`. The reported position after a seek is approximate by one audio block.
- `←`/`→` seek within the current track: 5 s per tap; while held, repeats only move the progress preview (yellow gauge,
  `跳转预览`) and the final target is committed 150 ms after the last repeat. `n` still advances the track.
  `Runtime::seek` ignores requests during a track switch or while the EOF tail/crossfade is draining, because `pos`
  still counts the previous track in that window.
- `CrossBridge` fills its delay line asymmetrically: while it is below the configured 1 s it emits at most half of each
  input block (retaining at least one sample), so playback starts with low latency and the line fills over time; once
  full it returns to 1:1. A seek clears the line, so playback resumes immediately and the latency ramps back instead of
  a silent refill. Crossfade gains stay precomputed for the full duration, but a short tail (a track ending while the
  line is filling) scales the table index to the actual tail, so A fades to 0 and B rises to 1 instead of the curve
  being truncated into a volume step.
- Fixed Windows VT input: crossterm 0.29 drops control characters (ESC, CR) from synthesized VT input, so `Enter` did
  nothing, arrows arrived as `[C` text and mouse reports leaked `[<...M` into the command line. Under VT input the TUI
  now reads raw stdin bytes on a dedicated reader thread and feeds `vtparse` directly; a lone ESC becomes the Esc key
  after a 50 ms timeout, and both CR and LF map to Enter.
- Fixed seeking before the first playback: `Player::try_seek` queued a `PlayCmd::Seek` on the capacity-1 transport
  channel. With the stream paused nothing consumed it, so the next `Play` failed with `Full` and playback never
  started. The seek no longer sends that command; the decoder seek and effect reset already run on the command thread.
- Mouse: the footer hint labels are now clickable buttons (quit, next, seek back/forward, play/pause, one-line scroll,
  volume, debug and command mode) in addition to the existing click-a-row-to-play behavior. `PlayList::jump_to` also
  removes the requested track from the periodic-shuffle cycle even when it is already the current track; a track that
  is not queued leaves the cycle untouched.

### Ring Buffers

- `split_ring_buffer` gained `resize` for `T: Copy` writers (`LockedWriter` / `UnlockedWriter`). Growing past the
  physical allocation reallocates A/B, blocks writers, copies the live range at the same logical positions (readers keep
  delivering data during the copy and only see "no data" in the tiny swap window), swaps the buffers and retires the old
  one until no operation is in flight; shrink or grow within the allocation only updates the logical capacity.
  `capacity()` exposes the current size, and `Kind::Resizing` reports a write rejected during a grow.

### File Finder Queue

- `file_finder` now uses a `ConcurrentRing<DirEntry>` for the directory work queue instead of `Mutex<VecDeque>` +
  `Condvar` + a running-thread counter. Workers block in `pop_wait()` and exit on `WritersClosed` / `AllWaiting`, so the
  manual increment/decrement/notify bookkeeping is gone. `FileFinder::new()` (plus `Default`) replaces the unit-struct
  literal, and the public helpers gained `#[must_use]`.

### Pack I/O

- `PackIO` now owns a `Box<dyn FileIo>` instead of a concrete `File`. The new `FileIo` trait (`pack_io/file.rs`,
  implemented for `File`: read/write/seek + `set_len`/`sync_data`/`lock`/`unlock`) lets the pack layer run on a non-`File`
  backend. Operations that move the file cursor (`manifest_data_block_read`, `set_pos_read`, and the manager's
  `load_pack_struct` / `load_pack_file_metadata`) take `&mut self`.
- `PackVirtualFile` moved from `pack_io/file.rs` to the new `pack_io/pack_file.rs` (`pack_io::file` now holds `FileIo`).
  `WBFilesPackManifest::file_mut()` returns `Option<&mut PackIO>`, and `file_gc_add` no longer returns a `Result`.

### Build

- The repository pins **nightly** in `rust-toolchain.toml` (`channel = "nightly"`), so every `cargo` command here uses
  nightly. `build_native.ps1` runs the native release build with `-Zbuild-std="std,panic_abort"` on top of
  `.cargo/native-config.toml`.

### Module Layout

- The music player engine moved into the library crate: `player`, `decoder_runtime`, `effect` (+ `runtime`/`resampler`/
  `cross_bridge`/`fade`), `play_list`, `event_dispatch` and `tools` now live under
  `water_ball_tool_lib/src/music_play/` and are exported as `water_ball_tool_lib::music_play`.
- The binary crate keeps only the terminal front-end: `src/command/music_play.rs` (clap arguments + TUI wiring),
  `src/command/music_play/runtime.rs` (playback runtime), `src/command/music_play/scanner.rs` (background tag scan) and
  `src/command/music_play/tui/`.
- `LoopMode` moved to the library with a `FromStr` implementation. The binary can no longer derive `clap::ValueEnum` for
  it (orphan rule), so `-m/--mode` is parsed via `value_parser`; accepted values are unchanged.
- The `debug-no-audio` feature is now defined on `water_ball_tool_lib` (where the cpal callback lives) and forwarded by
  the binary package, so `cargo build --features debug-no-audio` still works from the root.
- `cpal`, `symphonia`, `rubato` and `crossbeam-channel` moved from the binary package to the library; the binary keeps
  only `lofty` for tag reading.
- `cargo test --workspace` is now required to run the library test suite (a plain `cargo test` only covers the binary).

### Logging & Debugging

- Terminal log output is now asynchronous: `TerminalWriter` enqueues whole lines into a bounded channel (capacity 1024)
  drained by the `log-writer` thread, which prints them through `MultiProgress::suspend()`. Callers only `try_send`, so
  a paused debugger (or a stalled console) drops terminal lines instead of parking a worker thread inside logging while
  it holds the progress-bar write lock. File logging stays synchronous.
- Third-party `log` records are capped at `warn` (`log_filter`): `tracing-subscriber`'s default `tracing-log` bridge fed
  lofty's per-block `log::debug!` lines into that writer, and every line cleared and redrew all progress bars.
- `mp`: `add_track` no longer reads lofty tags, nor calls `next()`, while holding the `play_list` mutex — the first-track
  `next()` re-locked that same mutex on the same thread (self-deadlock).

### Track Collection

- `mp` only picks up files whose extension `lofty` recognizes (`FileType::from_path`) when walking a directory, so a
  library folder no longer pays a tag-read attempt per unrelated file. An explicitly passed file bypasses the filter and
  is always tried.

### Playlist Fixes

- `PeriodicShuffle` cycle boundary: when a cycle is refilled, the track that is still playing could be drawn first and
  repeat back to back. The refilled queue now moves the current index out of the front (new `avoid_immediate_repeat`
  helper). A single-track list cannot avoid the repeat and is left as-is.
- `Shuffle` picked `random_range(0..len)`, but track indices are 1-based (`get_current_track` uses `current_index - 1`):
  index 0 underflowed and the last track could never play. The range is now `1..=len`, so a single-track list plays.

### Playlist Editing & Previous Track

- `PlayList` gained a bounded navigation history (`VecDeque`, pre-allocated to `HISTORY_CAP = 1024`, starting empty).
  Only `next` writes it: it pushes the index it leaves whenever the track actually changes (a `RepeatOne` automatic loop
  stays put and records nothing), evicting the oldest entry when full.
- `PlayList::prev()` pops the history and returns the previous track with a fresh `next_id` (so the outgoing track's
  buffered EOF cannot trigger an automatic advance on the new one). It returns `None` at the start of the history; there
  is no redo path, so a following `next` is recalculated from the current mode.
- In `PeriodicShuffle`, `prev` also drops the landing track from the current cycle queue, so it cannot play twice in the
  same cycle.
- New editing API: `add_track` / `add_tracks` (append) and `insert_track` / `insert_tracks` (1-based position, clamped
  to `1..=length+1`). The batch variant is the only implementation; the single-track one forwards to it.
- A middle insertion shifts `current_index`, every history entry and every cycle entry at or after the insertion point;
  appending at the end touches none of them. In `PeriodicShuffle` the new tracks are queued for the current cycle when
  one is in progress.

### Documentation

- `AGENTS.md` now holds conventions only; the per-module architecture notes moved to `ARCHITECTURE.md`, which gained a
  "Ring Buffer Modules" chapter (§12) covering `concurrent_ring` and `split_ring_buffer`.

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

### Decoder Error Recovery

- The decode loop retries transient IO errors (`DecoderRuntime::is_retryable_io_error`: `Interrupted`, `WouldBlock`,
  `TimedOut`, connection reset/abort, and platform resource-exhaustion codes such as Windows 1450
  `ERROR_NO_SYSTEM_RESOURCES`) up to 5 times with a 200 ms delay before treating them as failures.
- Errors that cannot be retried, or whose retries are exhausted, no longer kill the decode thread: the loop emits the
  `DecoderEvent` error, takes the `DecoderSession` as a pair, clears the pending output buffer and returns to the condvar
  wait. The thread stays alive, so a later track change plays again instead of leaving the decoder permanently silent
  (observed in practice after `next_packet` failed with `os error 1450`).
- `DecoderRuntime` exposes an atomic `poisoned` flag (`is_poisoned`), set by those unrecoverable errors and cleared
  automatically by a successful `set_decoder_path`, so a forgotten front-end cleanup cannot turn the next play command
  into a premature track change.
- `mp` consumes decoder error events through a `DecoderEventChannelSink` (channel thread; it pauses playback and wakes
  the UI thread). While poisoned, the progress bar shows the `异常暂停中` prefix and a red `Error` line between the song
  info and the time-metrics block, and `Play`/`P` changes track first while poisoned and always resumes playback
  afterwards.

### Ring Half-Close (Step 1)

- `ConcurrentRing` now fails blocking calls fast instead of blocking forever: every blocking method (`push_wait`,
  `write_wait`, `write_all` and `pop_wait`, `read_wait`, `read_exact`) registers the calling handle's direction, and
  the handles' `Clone`/`Drop` keep a per-direction count. Once the last handle of one direction disappears, a blocking
  call on the opposite side returns the new `ReadersClosed` / `WritersClosed` error; a newly registered handle of that
  direction restores it. The decision is fully automatic through the methods plus `Clone`/`Drop` — no reader/writer
  handle type, no manual close/reopen and no extra "closed" booleans (the counters are the state). A ring whose handles
  never called a blocking method is never reported closed.

### Pre-Effect Runtime Error Handling (Step 2)

- The pre-effect runtime thread no longer exits on a runtime error: a ring error resets the effect chain with the
  `Skip` discontinuity, emits `PreEffectRuntimeEventErrorKind::Runtime`, backs off 10 ms and keeps consuming (a closed
  write side therefore parks instead of spinning); a hard output-write error is reported the same way and the block is
  dropped. Only the `stop` flag ends the loop.
- `Runtime::create` registers a `PreEffectRuntimeEventChannelSink` that only logs these errors (playback may continue,
  no pause).

### Crash Watchdog (Step 3)

- New library module `music_play/watchdog.rs`: watched threads own a `ThreadExitGuard` whose `Drop` (panic unwind
  included) marks their slot in `WatchNotify` and wakes a single watchdog thread on a `Condvar` — no
  `JoinHandle::is_finished()` polling and no second thread.
- `Player::new` injects the notifier into the decoder and pre-effect runtimes and spawns `player-watchdog`. On a thread
  exit it pauses playback (no automatic resume), sets the decoder poison flag so `Play` recovers by changing track, and
  restarts the thread; `DecoderRuntime::run` / `PreEffectRuntime::run` now reclaim a finished `JoinHandle` before
  respawning.
- Pre-effect restart rebuilds the whole effect chain: `PreEffectRuntime::set_chain_factory` (registered by
  `Player::new`) + `rebuild_effects()` / `set_effects()`, because the old effects may already be poisoned. The stale
  input ring is cleared before the restart.
- `Player::try_stop` shuts the watchdog down first and joins it last.

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
- `split_ring_buffer`: blocking handoffs could rarely deadlock. The publisher checked "are there waiters?" outside  `wait_mutex` right after publishing the new position, while the waiter registered itself and then re-read that
  position; x86 StoreLoad reordering let both miss each other, parking the peer forever (reproduced ~1 in 200 runs of
  the cross-capacity blocking tests). Both sides now execute `fence(SeqCst)` around the handshake, so a skipped
  wakeup and a stale position read cannot coincide. The hot path still takes no lock, so the realtime audio callback
  remains lock-free.
- `concurrent_ring`: `ConcurrentRingGuard` (`ring.lock()`) never woke blocked peers, so a thread parked in `push_wait` /
  `pop_wait` could sleep forever when the only progress came from guard operations. Waking inside the guard is
  impossible (it needs `wait_lock`, and the lock order is `wait_lock → write_lock → read_gate.write`), so the guard now
  releases both locks in `Drop` and only then wakes writers and readers.

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
