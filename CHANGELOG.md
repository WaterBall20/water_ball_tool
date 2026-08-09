# Changelog

> **Language**: English | [简体中文](docs/zh_CN/CHANGELOG.md)

## [Unreleased] — 2026-07-30

### Doc Comment Normalization

- `cargo doc --no-deps` warnings reduced to zero (fixed 2 `SearchWarning` instances being parsed as unclosed HTML tags)
- `FileInfo::new()`, `PackVirtualFile::get_modified()`, `PackVirtualFile::set_modified()`: added missing public API doc comments
- `bytes_len_to_string()`, `path_to_string_vec()`, `path_remove_head()`: added algorithm notes and runnable doc-test examples
- `ManifestDataBlock::next_ver()` / `ver_is_older()` / `get_ver()` / `get_block_len()` / `get_block_len_us()`: added internal algorithm doc comments
- `bytes_len_to_string()` format string fixed `"{}"` → `"{:.2}"` to match the documented two-decimal output
- `path_remove_head()` fixed prefix comparison bug: `path_vec > head_vec` (lexicographic) → `path_vec[..head_vec.len()] == head_vec` (correct prefix matching)

### Code Cleanup & Format Normalization

- `src/wb_files_pack/*.rs`: globally unified `use` import ordering (std → third-party → crate-internal), merged scattered imports
- Reformatted multi-line expressions: `match` arms, method chains, tuple destructuring; eliminated line-too-long warnings
- Added `#[cfg(test)]` gating to test-only `assert!` / `debug_assert!` usage, removing unused warnings in release builds
- `src/gakumasu/data.rs`: fixed multi-line block comment alignment
- Fixed missing trailing newlines in several files (`simulator.rs`, `file_hash.rs`, `pack_io.rs`)

### WBFP Manager Refactoring

- `src/wb_files_pack/manager.rs`: `WBFPManagerRun.write_lock` refactored into standalone substruct `WBFPManagerRunLock { lock, path, file }` for better cohesion
- `WBFPManagerRunLock` implements `Clone` (`file` field set to `None` on clone to avoid duplicated file descriptors)
- `PackLockInfo.run_lock` type promoted from `bool` to `WBFPManagerRunLock`, carrying full lock state
- `src/wb_files_pack/data.rs`: `WBFilesPackManifest::file()` return type changed from `&Option<PackIO>` to `Option<&PackIO>`, more idiomatic Rust API
- `WBFPManager::save_all()` visibility adjusted from `pub(super)` to `pub(crate)`

### PackVirtualFile API Simplification

- `get_len()` / `get_modified()` removed the `Result` wrapper, returning `u64` / `u128` directly — internal lock acquisition no longer produces a recoverable error
- Lock poisoning recovery unified to `std::sync::PoisonError::into_inner`, replacing the closure `|e| e.into_inner()`
- `check_sync_compat()` uses `|` pattern to merge read-only/write-only match branches, eliminating redundant code

### Command-Layer Extraction

- `src/command/wbfp.rs`: unpack work loop extracted into standalone `wpfp_u_work()` function
- Hash-verify work loop extracted into `wpfp_h_work()` function
- Eliminated duplicate queue-consumption logic between unpack and hash-verify

### Conditional Compilation Optimization

- `gakumasu` module (WIP game simulator) gated behind `#[cfg(debug_assertions)]`; no longer compiled in release builds
- `net_server` module (WIP network server) likewise gated behind `#[cfg(debug_assertions)]`
- `src/lib.rs` public API more lean in release

### Error Handling Unification

- `src/wb_files_pack/error.rs`: all `Display` implementations use inline format variables `{e}` `{msg}`, eliminating redundant `format!` calls
- Removed unnecessary `.into()` calls across the codebase (auto-converted via type inference)

### Misc

- `.vscode/launch.json`: simplified debug config names, added `--include-ignored` test-run config
- `src/tools.rs`: merged nested `use std::path` declarations
- `ManagerSync`: `access_mode` field initialization moved earlier to match declaration order
- `ManagerSync::create_new()`: added `path.try_exists()` pre-check for clearer existence semantics

### Warning Suppression Removal

- Removed all 5 lint-suppression attributes across the codebase: `#![allow(clippy::unwrap_used)]` in `src/file_finder/test.rs` and `src/wb_files_pack/pack_io/file.rs`, plus `#[allow(clippy::too_many_arguments)]` on 3 functions in `src/file_finder.rs`
- Added `TestTool::expect_ok()` helper (`src/tools.rs`) for tests — a non-panicking unwrap replacement compatible with the crate-wide `#![deny(clippy::unwrap_used)]`
- Rewrote all 30 `unwrap()`/`expect()` calls in test code to `TestTool::expect_ok()` or an explicit `match` (thread `join()` errors are `Box<dyn Any + Send>`, not `Display`)
- AGENTS.md: added "Warning-suppression attributes are FORBIDDEN" rule — `#[allow(...)]`, `#![allow(...)]`, `#[expect(...)]` and any other lint-suppression attribute MUST NOT be added to production or test code
