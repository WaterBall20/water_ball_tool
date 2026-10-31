# 更新日志

> **语言**: [English](../CHANGELOG.md) | 简体中文

## [Unreleased] — 2026-10-31（第二轮）

### 线程创建与错误传播

- `ff`/`wbfp` 命令模块与音乐播放器的所有工作线程改用 `thread::Builder::new().name(...).spawn(...)`；spawn 失败作为错误向上
  传播（`ff`/`wbfp` 共 9 处）。AGENTS.md 规范禁止会 panic 的 `std::thread::spawn`（操作系统无法创建线程时会 panic）
- `DecoderRuntime::run()` 与 `PreEffectRuntime::run()` 返回 `Result`；spawn 失败会中止 `Player::new()`，不再只记日志后继续
- 事件通道 sink 构造器（`PlayerEventChannelSink::new`、`DecoderEventChannelSink::new`、
  `PreEffectRuntimeEventChannelSink::new`）返回 `std::io::Result`；`Player::new()` / `Runtime::create()` 会传播该错误。
  sink trait 中的 `new` 已移除；DirectSink 的构造不返回错误
- `DecoderRuntime::stop()` 在 `send_skip` 失败时仍会 join 解码线程，然后返回第一个错误

### 解码器会话配对

- 解码器的 engine（格式读取器 + 解码器）与 data（轨道元数据）合并为单个 `DecoderSession`，存放在一个
  `Arc<Mutex<Option<DecoderSession>>>` 中；换曲时在同一把锁内整体替换/取出会话
- 删除旧的 engine/data 两个独立 `Option` 以及「有 engine 但缺 data」的分支
- 解码循环错误策略：真实运行期错误（next_packet 失败、环形缓冲区真实写入失败、EOF 写入失败、非帧级致命解码错误）会发出
  `DecoderEvent` 错误事件并停止解码线程；单帧 `DecodeError` 上报后跳过继续；停止/切曲/寻道时的 `WriteRejected`
  属于正常控制流

### 逻辑错误使用 `unreachable!`

- `wb_files_pack`（`manager/file_ops.rs`、`manager/metadata.rs`、`pack_io/file_handle.rs`）：所有逻辑错误 `panic!`
  分支改为 `unreachable!`
- `metadata.rs`：`open_virtual_file`/`create_virtual_file` 传入目录路径属于运行期错误，改为返回 `PackFileError`，
  不再中止进程
- `ff`/`wbfp`：每工作线程进度条池耗尽属于不变量破坏，改用 `unreachable!`
- AGENTS.md 新增两条错误处理规范：线程创建失败必须返回错误；逻辑错误必须使用 `unreachable!`（运行期失败绝不能改写成
  `unreachable!`）

### 音乐播放器（`mp`）

- `-v/--volume` 超出 0.0–1.0 时改为在开始播放前返回参数错误（原先会 panic）；帮助文本标注范围为 0.0–1.0

## [Unreleased] — 2026-10-31

### workspace 拆分

- 根 `Cargo.toml` 现为 cargo workspace（`resolver = "3"`，成员 `water_ball_tool_lib`）；根包保留 `src/` 下的
  `water_ball_tool` 二进制 crate
- 库代码迁移到 `water_ball_tool_lib/src/`：`file_finder`、`wb_files_pack`、`tools`、`gakumasu`（仅 debug 构建）
- 新增库模块：`concurrent_ring`（可并发初始化的环形缓冲区）与 `split_ring_buffer`（读写分离有锁/无锁环形缓冲区）——音乐播放器的
  解码/效果/音频管线均使用它们
- 二进制代码改为通过 `water_ball_tool_lib::*` 引用库（`command.rs`、`command/ff.rs`、`command/wbfp.rs`）

### 音乐播放器（`mp`）

- 新增终端音乐播放器子命令 `mp`（模块 `music_play`，枚举变体 `MP`，`src/command/music_play.rs`）：播放音频文件/目录，用
  `lofty` 读取标签，构建 `PlayList`，并接受 stdin 命令 `next`/`n`、`Play`/`P`、`pause`/`p`、`exit`
- 参数：位置参数 `files...`、`-v/--volume`（0.0–1.0，默认 0.75）、`--verbose`（仅长选项）、`-m/--mode`
  （`sequential-loop`、`sequential`、`repeat-one`、`shuffle`、`periodic-shuffle`）
- 模块：`player`、`decoder_runtime`、`effect`（`resampler`、`cross_bridge`、`fade`）、`play_list`、`runtime`、
  `event_dispatch`、`tools`；`tui/` 仍在开发中，尚未接入命令

### 音频实时线程

- cpal 回调不再加锁、不调用事件 sink、不写日志；只把事件与日志记录 `try_send` 到有界通道（容量 1024，丢弃消息计数）
- 新增 `player-event-dispatch` 线程（`command/music_play/event_dispatch.rs`）负责加锁、调用 sink 和真正输出 `tracing`
  日志；流错误也在该线程输出
- 曲目结束改为投递 `TrackEnded` 事件，而非在回调内直接调用 sink
- `Player::try_stop` 按顺序回收全部线程：暂停音频流 → 停止前效果器运行时（唤醒阻塞在 `push_wait` 的生产者）→ 停止解码线程 →
  停止事件分发线程 → join `rx_cmd` 线程（幂等）

### 日志

- 用 `TerminalWriter`/`FileWriter` 包装替换旧的进度条 `Write` 适配器；`init_global_logging` 现在接收
  `(&mp, module)` 并返回 `Option<PathBuf>`，`multi_progress_init(module)` 创建每个命令的 `MultiProgress`
  并安装 subscriber
- Debug 构建：仅终端输出，级别 `debug`。Release 构建：终端级别 `info` + 级别 `debug` 的日志文件
  （`<ProjectDirs data_local_dir>/<crate>/logs/<module>/<module>-<时间戳>.log`）；若无法创建文件，回退为仅终端的 `debug`
  级别输出

### 文件搜索哈希（`ff`）

- `[OUT_PATH]` 之后新增可选多值位置参数 `[HASH]...`：`all`、`md5`、`sha256`、`sha512`、`sha1`、`blake3`、`blake2b`、
  `crc32`（别名 `*`、`m5`、`s256`、`s512`、`s1`、`b3`、`b2b`、`crc`）
- 逐文件哈希附加到 `FileInfo` 并写入 JSON 输出
- 开启哈希时文件发现被强制为单线程；哈希计算本身由 `thread_count` 个工作线程执行
- 选择 MD5/SHA-1 时会记录一条不安全警告

### 代码规范

- `water_ball_tool_lib/src/lib.rs`：`#![cfg_attr(not(test), deny(clippy::expect_used))]`——生产构建中 `expect` 变为
  deny（测试目标仍保留 warn）
- `command.rs`：新增 `progress_style(template)` 辅助函数——模板非法时记录错误并回退默认样式，替代 `expect` 恐慌

### Bug 修复

- WBFP 擦除（`OverwriteStrategy::Random` / `Dod5220`）：写偏移未随块推进，大于 `DATA_DATA_BLOCK_LEN` 的数据块只覆写首块；
  现在所有遍都定位到 `pos + write_len`
- `ff` 哈希：进度通道发送失败时跳过了 `hash_type.reset()`，污染下一文件的哈希；现在所有路径都会 finalize/reset
- 音乐播放器退出路径补全：音频流、前效果器运行时、解码线程、事件分发线程与 `rx_cmd` 线程都会在 `try_stop` 中停止/join，
  不再泄漏到进程退出
- 音乐播放器音频回调：事件 sink 调用与日志输出移到分发线程（修复实时抖动/优先级反转）

### 构建

- `.cargo/native-config.toml` 必须手动传入（`cargo --config .cargo/native-config.toml build --release`）才能启用
  `target-cpu=native` 并把产物输出到 `target/native/`
- 新增 `debug-no-audio` feature：音频回调跳过环形缓冲区读取、后置效果器与音量缩放，用于无音频的播放控制快速调试

## [Unreleased] — 2026-07-30

### 文档注释规范化

- `cargo doc --no-deps` 警告归零（修复 2 处 `SearchWarning` 被解析为未闭合 HTML 标签的问题）
- `FileInfo::new()`、`PackVirtualFile::get_modified()`、`PackVirtualFile::set_modified()`：补全缺失的公共 API 文档注释
- `bytes_len_to_string()`、`path_to_string_vec()`、`path_remove_head()`：增加算法说明及可执行示例文档测试
- `ManifestDataBlock::next_ver()` / `ver_is_older()` / `get_ver()` / `get_block_len()` / `get_block_len_us()`：增加内部算法文档注释
- `bytes_len_to_string()` 格式化字符串修正 `"{}"` → `"{:.2}"`，确保两位小数输出与文档描述一致
- `path_remove_head()` 修复前缀比较 bug：`path_vec > head_vec`（字典序）→ `path_vec[..head_vec.len()] == head_vec`（正确的前缀匹配）

### 代码清理与格式化规范化

- `src/wb_files_pack/*.rs`: 全局统一 `use` 导入顺序（标准库 → 第三方 → crate 内部），合并分散导入
- 多行长表达式格式化：`match` 臂、方法链、元组解构，消除行超长警告
- 为仅测试环境使用的 `assert!` / `debug_assert!` 添加 `#[cfg(test)]` 门控，消除 release 构建中的 unused 警告
- `src/gakumasu/data.rs`: 修复多行块注释对齐
- 修复多个文件末尾缺失换行符问题（`simulator.rs`, `file_hash.rs`, `pack_io.rs`）

### 包文件管理器重构

- `src/wb_files_pack/manager.rs`: `WBFPManagerRun.write_lock` 重构为独立子结构体
  `WBFPManagerRunLock { lock, path, file }`，提升内聚性
- `WBFPManagerRunLock` 实现 `Clone`（`file` 字段在克隆时置 `None`，避免文件描述符重复）
- `PackLockInfo.run_lock` 类型从 `bool` 提升为 `WBFPManagerRunLock`，传递完整锁状态
- `src/wb_files_pack/data.rs`: `WBFilesPackManifest::file()` 返回类型从 `&Option<PackIO>` 改为 `Option<&PackIO>`，更符合
  Rust API 惯例
- `WBFPManager::save_all()` 可见性从 `pub(super)` 调整为 `pub(crate)`

### PackVirtualFile API 简化

- `get_len()` / `get_modified()` 移除 `Result` 包装，直接返回 `u64` / `u128` —— 内部锁获取不再产生可恢复错误
- 锁毒性恢复统一使用 `std::sync::PoisonError::into_inner` 替代闭包 `|e| e.into_inner()`
- `check_sync_compat()` 使用 `|` 模式合并只读/只写匹配分支，消除冗余代码

### 命令层解耦重构

- `src/command/wbfp.rs`: 解包工作循环提取为 `wpfp_u_work()` 独立函数
- 哈希校验工作循环提取为 `wpfp_h_work()` 独立函数
- 消除解包与哈希校验中队列消费逻辑的重复实现

### 条件编译优化

- `gakumasu` 模块（开发中游戏模拟器）门控为 `#[cfg(debug_assertions)]`，release 构建不再编译
- `net_server` 模块（开发中网络服务）同样门控为 `#[cfg(debug_assertions)]`
- 库 crate（`water_ball_tool_lib`）公共 API 在 release 中更精简

### 错误处理统一

- `src/wb_files_pack/error.rs`: `Display` 实现全部改用内联格式变量 `{e}` `{msg}`，消除冗余 format! 调用
- 代码库整体移除不必要的 `.into()` 调用（通过类型推导自动转换）

### 杂项

- `.vscode/launch.json`: 调试配置名称简化，增加 `--include-ignored` 测试运行配置
- `src/tools.rs`: 合并嵌套的 `use std::path` 路径声明
- `ManagerSync`: `access_mode` 字段初始化位置提前，与声明顺序一致
- `ManagerSync::create_new()`: 增加 `path.try_exists()` 前置检查，使存在性判断逻辑更明确

### 移除警告忽略代码

- 移除代码库中全部 5 处 lint 抑制属性：`src/file_finder/test.rs` 与 `src/wb_files_pack/pack_io/file.rs` 中的
  `#![allow(clippy::unwrap_used)]`，以及 `src/file_finder.rs` 中 3 个函数上的 `#[allow(clippy::too_many_arguments)]`
- `src/tools.rs` 新增 `TestTool::expect_ok()` 测试辅助函数——配合 crate 级 `#![deny(clippy::unwrap_used)]` 的非 panic
  unwrap 替代方案
- 测试代码中全部 30 处 `unwrap()`/`expect()` 调用改写为 `TestTool::expect_ok()` 或显式 `match`（线程 `join()` 错误为
  `Box<dyn Any + Send>`，非 `Display`）
- AGENTS.md：新增"禁止忽略警告的属性（FORBIDDEN）"规则——`#[allow(...)]`、`#![allow(...)]`、`#[expect(...)]` 及任何其他 lint
  抑制属性不得添加到生产或测试代码中
