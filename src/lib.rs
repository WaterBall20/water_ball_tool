//! 水球工具 / Water Ball Tool
//!
//! 一个多功能命令行工具，提供文件搜索、水球包文件（WBFP）管理等功能。
//! A multi-purpose CLI tool providing file search, Water Ball Files Pack (WBFP) management, and more.

//#![deny(clippy::unwrap_used)]
/// 文件搜索器模块 / File finder module
pub mod file_finder;
/// 水球包文件模块（容器文件格式）/ Water Ball Files Pack module (container file format)
pub mod wb_files_pack;

/// 通用工具函数模块 / General utility functions module
pub mod tools;

/// 游戏模拟器模块（开发中）/ Game simulator module (WIP)
pub mod gakumasu;
