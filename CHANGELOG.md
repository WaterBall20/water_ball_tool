# 更新日志 / Changelog

## [Unreleased] — 2026-07-30

### 文档注释规范化 / Doc Comment Normalization

- `cargo doc --no-deps` 警告归零（修复 2 处 `SearchWarning` 被解析为未闭合 HTML 标签的问题）
- `FileInfo::new()`、`PackVirtualFile::get_modified()`、`PackVirtualFile::set_modified()`：补全缺失的公共 API 文档注释
- `bytes_len_to_string()`、`path_to_string_vec()`、`path_remove_head()`：增加算法说明及可执行示例文档测试
- `ManifestDataBlock::next_ver()` / `ver_is_older()` / `get_ver()` / `get_block_len()` / `get_block_len_us()`：增加内部算法文档注释
- `bytes_len_to_string()` 格式化字符串修正 `"{}"` → `"{:.2}"`，确保两位小数输出与文档描述一致
- `path_remove_head()` 修复前缀比较 bug：`path_vec > head_vec`（字典序）→ `path_vec[..head_vec.len()] == head_vec`（正确的前缀匹配）

### 代码清理与格式化规范化 / Code Cleanup & Format Normalization

- `src/wb_files_pack/*.rs`: 全局统一 `use` 导入顺序（标准库 → 第三方 → crate 内部），合并分散导入
- 多行长表达式格式化：`match` 臂、方法链、元组解构，消除行超长警告
- 为仅测试环境使用的 `assert!` / `debug_assert!` 添加 `#[cfg(test)]` 门控，消除 release 构建中的 unused 警告
- `src/gakumasu/data.rs`: 修复多行块注释对齐
- 修复多个文件末尾缺失换行符问题（`simulator.rs`, `file_hash.rs`, `pack_io.rs`）

### 包文件管理器重构 / WBFP Manager Refactoring

- `src/wb_files_pack/manager.rs`: `WBFPManagerRun.write_lock` 重构为独立子结构体 `WBFPManagerRunLock { lock, path, file }`，提升内聚性
- `WBFPManagerRunLock` 实现 `Clone`（`file` 字段在克隆时置 `None`，避免文件描述符重复）
- `PackLockInfo.run_lock` 类型从 `bool` 提升为 `WBFPManagerRunLock`，传递完整锁状态
- `src/wb_files_pack/data.rs`: `WBFilesPackManifest::file()` 返回类型从 `&Option<PackIO>` 改为 `Option<&PackIO>`，更符合 Rust API 惯例
- `WBFPManager::save_all()` 可见性从 `pub(super)` 调整为 `pub(crate)`

### PackVirtualFile API 简化 / PackVirtualFile API Simplification

- `get_len()` / `get_modified()` 移除 `Result` 包装，直接返回 `u64` / `u128` —— 内部锁获取不再产生可恢复错误
- 锁毒性恢复统一使用 `std::sync::PoisonError::into_inner` 替代闭包 `\|e\| e.into_inner()`
- `check_allocator_compat()` 使用 `|` 模式合并只读/只写匹配分支，消除冗余代码

### 命令层解耦重构 / Command-Layer Extraction

- `src/command/wbfp.rs`: 解包工作循环提取为 `wpfp_u_work()` 独立函数
- 哈希校验工作循环提取为 `wpfp_h_work()` 独立函数
- 消除解包与哈希校验中队列消费逻辑的重复实现

### 条件编译优化 / Conditional Compilation Optimization

- `gakumasu` 模块（开发中游戏模拟器）门控为 `#[cfg(debug_assertions)]`，release 构建不再编译
- `net_server` 模块（开发中网络服务）同样门控为 `#[cfg(debug_assertions)]`
- `src/lib.rs` 公共 API 在 release 中更精简

### 错误处理统一 / Error Handling Unification

- `src/wb_files_pack/error.rs`: `Display` 实现全部改用内联格式变量 `{e}` `{msg}`，消除冗余 format! 调用
- 代码库整体移除不必要的 `.into()` 调用（通过类型推导自动转换）

### 杂项 / Misc

- `.vscode/launch.json`: 调试配置名称简化，增加 `--include-ignored` 测试运行配置
- `src/tools.rs`: 合并嵌套的 `use std::path` 路径声明
- `Allocator`: `access_mode` 字段初始化位置提前，与声明顺序一致
- `Allocator::create_new()`: 增加 `path.try_exists()` 前置检查，使存在性判断逻辑更明确
