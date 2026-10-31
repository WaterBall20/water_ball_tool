//! 水球工具 / Water Ball Tool
//!
//! 一个多功能命令行工具，提供文件搜索、水球包文件（WBFP）管理等功能。
//! A multi-purpose CLI tool providing file search, Water Ball Files Pack (WBFP) management, and more.

#![deny(clippy::unwrap_used)]
#![warn(clippy::expect_used)]
//AI=== 生产代码禁止 expect：非 test 构建提升为 deny（测试目标保留 warn，既有测试 expect 迁移另行处理）
// 生产构建禁用 expect；测试目标仍为 warn，避免与此前测试代码冲突。
// `expect` is denied for non-test builds; test targets keep `warn` until the
// pre-existing test expectations are migrated.
#![cfg_attr(not(test), deny(clippy::expect_used))]
//AI_END===
#![deny(clippy::undocumented_unsafe_blocks)]
#![deny(clippy::unnecessary_safety_doc)]
/// 文件搜索器模块 / File finder module
pub mod file_finder;
/// 水球包文件模块（容器文件格式）/ Water Ball Files Pack module (container file format)
pub mod wb_files_pack;

/// 通用工具函数模块 / General utility functions module
pub mod tools;

//AI=== 补充环形缓冲区模块文档（公共 API 文档同步）
/// 并发环形缓冲区模块 / Concurrent ring buffer module
pub mod concurrent_ring;
/// 读写分离环形缓冲区模块 / Split read-write ring buffer module
pub mod split_ring_buffer;
//AI_END===
/// 游戏模拟器模块（开发中）/ Game simulator module (WIP)
#[cfg(debug_assertions)]
pub mod gakumasu;
