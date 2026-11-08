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
cargo test --workspace         # skip long-time tests (#[ignore] handled automatically)
cargo test --workspace -- --include-ignored # include long-time tests (full CI suite on main-branch pushes)
./target/release/water_ball_tool -h
```

- `wbfp` runs a TUI when stdout and stdin are terminals; `--no-tui` (or a non-TTY stdout/stdin) keeps the classic progress-bar output.
  Without a subcommand it opens the file manager: local filesystem on the left, an optional pack in the middle and
  preview on
  the right. `wbfp <pack>` opens that pack immediately; bare `wbfp` starts with the middle pane hidden. This form is
  TUI-only (`--no-tui` and non-TTY input are rejected). `wbfp browse <pack>` remains the read-only single-pane browser,
  and the pack/unpack/verify subcommands keep their preview-only layout.

## Build options

- The repository is a **workspace**: the root `Cargo.toml` defines the `water_ball_tool` binary package and lists
  `water_ball_tool_lib` as a member. `cargo build` from the repository root builds both.
- `.cargo/native-config.toml` is **not** applied automatically (cargo only auto-loads files named `config.toml`). Pass
  it explicitly: `cargo --config .cargo/native-config.toml build --release`. It compiles with
  `-C target-cpu=native` and redirects artifacts to `target/native/` (binary at
  `target/native/release/water_ball_tool`). Do not use a native build as a portable distribution.
- The repository pins **nightly** in `rust-toolchain.toml` (`channel = "nightly"`), so every `cargo` command here runs on
  nightly. `build_native.ps1` runs the native release build with `-Zbuild-std="std,panic_abort"` on top of
  `.cargo/native-config.toml`.
- The `debug-no-audio` feature is a debugging aid for the `mp` command: the cpal callback skips ring-buffer reads,
  post-effect processing, and volume scaling, so playback control can be debugged quickly without real audio. It
  produces no usable audio. The feature is defined on `water_ball_tool_lib` (the cpal callback lives there) and the
  binary package forwards it, so `cargo build --features debug-no-audio` from the root still works.

## Testing conventions

- **Never swallow test output**: run tests directly (`cargo test ...`) and keep the full stream visible. Do not pipe the
  command through filters such as `Select-String`, `grep`, or `rg` that discard progress and failure details; a filtered
  run that hangs or times out leaves no evidence. If the output is too large, redirect it to a file and inspect that
  file instead of dropping the stream.
- Slow tests are marked `#[ignore = "longtime"]`. Skip them with `cargo test --workspace` (skipped by default). Use
  `--include-ignored` for the full suite on main-branch CI pushes. `--workspace` is required: `water_ball_tool_lib` is
  a workspace member, so a plain `cargo test` only covers the binary crate.
- `#[ignore = "longtime"]` is allowed **only** for tests that are guaranteed to exceed 60 seconds. Never mark a test
  ignored because it is flaky, occasionally hangs, or is inconvenient: fix or split the test instead.
- `ff` and the classic `wbfp` path initialize logging through `crate::multi_progress_init(module)`, so tests that call
  their handlers get a working `MultiProgress` without extra setup. The `wbfp` TUI and `mp` install the TUI channel
  writer (`tui::channel_writer::init_logging`), so tests must not call `wbfp::run`/`music_play::args` expecting a
  terminal. Re-registering the subscriber is rejected by `try_init` and only logged. Tests use `wbfp::commands`, which
  always takes the classic path.
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
    - Randomized tests run ONLY in full-suite mode (`cargo test --workspace -- --include-ignored`), gated with
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

## Architecture

Module-level design lives in [ARCHITECTURE.md](ARCHITECTURE.md); read the chapter for the module you are changing —
`ff` §6, `mp` §7, `wbfp` §8, progress bars §9, logging §10, tools §11, ring buffers §12 — instead of duplicating it in
this file. Binary format specifications live in `docs/en_US/` (Chinese: `docs/zh_CN/`).

Rules every change must respect, which module docs do not restate:

- `src/command.rs` is private to the binary crate; binary code reaches the library as `water_ball_tool_lib::<module>`.
- The realtime audio callback never locks, calls sinks, or writes logs, and `Player::try_stop` keeps its fixed shutdown
  order (ARCHITECTURE §7.3, §7.5).
- Module details belong in ARCHITECTURE.md or the module's `//!` docs — not in this specification file.

## Code change markers

- `//AI===` / `//AI_END===` are code change-region markers.
- Every modified, added, or removed code segment MUST be wrapped in paired markers (`//AI===` start, `//AI_END===` end).
- The start marker may carry a one-line note describing the change.
- Markers wrap code only, never documentation: do NOT put a marker inside or around `///` / `//!` doc comments (a plain
  `//` line between doc lines silently drops the following doc text). A doc-comment-only change needs no marker at all;
  when code and its doc comment change together, mark only the code.

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
