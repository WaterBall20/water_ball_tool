# AGENTS.md — 水球工具

> **语言**: [English](../AGENTS.md) | 简体中文

Rust 2024 workspace CLI 工具箱。三个命令：`ff`（并行文件搜索器，可选哈希）、`wbfp`（自定义二进制打包格式）和
`mp`（终端音乐播放器）。

## 命令

```bash
cargo build --release          # 发布构建
cargo build --features debug-no-audio # 跳过音频处理的调试构建（见「构建选项」）
cargo --config .cargo/native-config.toml build --release # 本机优化构建 → target/native
cargo check                    # 仅编译检查（快速）
cargo clippy                   # 代码检查
cargo test                     # 跳过长时间测试（#[ignore] 自动处理）
cargo test -- --include-ignored # 包含长时间测试（主分支推送时全量 CI）
./target/release/water_ball_tool -h
```

## 构建选项

- 仓库是一个 **workspace**：根 `Cargo.toml` 定义 `water_ball_tool` 二进制包，并把 `water_ball_tool_lib` 列为成员。
  在仓库根执行 `cargo build` 会同时构建两者。
- `.cargo/native-config.toml` **不会**自动生效（cargo 只自动加载名为 `config.toml` 的文件）。需要显式传入：
  `cargo --config .cargo/native-config.toml build --release`。它使用 `-C target-cpu=native` 编译，并把产物重定向到
  `target/native/`（二进制位于 `target/native/release/water_ball_tool`）。不要把 native 构建当作可分发版本。
- `debug-no-audio` feature 是 `mp` 命令的调试辅助：cpal 回调会跳过环形缓冲区读取、后置效果器处理和音量缩放，便于在没有真实
  音频的情况下快速调试播放控制。它不产生可用的音频输出。

## 测试规范

- 慢速测试标记为 `#[ignore = "longtime"]`。`cargo test` 默认跳过它们。主分支 CI 推送时使用 `--include-ignored` 运行全量套件。
- 子命令处理函数通过 `crate::multi_progress_init(module)` 自行初始化日志；调用处理函数的测试（`ff::args`、
  `wbfp::commands`、`music_play::args`）因此无需额外设置即可获得可用的 `MultiProgress`。重复注册 subscriber 会被
  `try_init` 拒绝并仅记录日志。
- 测试临时目录位于 `./temp/test/`（gitignore）。
- 测试开始用 `TestTool::prepare_test_dir(dir)`、测试结束用 `TestTool::cleanup_test_dir(dir)`（均来自
  `water_ball_tool_lib/src/tools.rs`；两者容忍 `NotFound`；清理遇到非预期错误会 panic）。
  `TestTool::remove_test_pack_files(path)` 在打包测试后删除 `.pack`/`.wbm`/`.lock` 文件。
- 跨平台测试使用 `#[cfg(unix)]` / `#[cfg(windows)]` 创建符号链接及平台相关路径。

### 测试目录与资源生命周期

- **测试绝不能互相依赖**：每个测试完全自包含；其消费和产生的目录与所有其他测试的目录互不相交。
- **资源目录布局**：
    - 测试夹具——测试 *需要*的文件：`./resources/test/`
    - 测试输出——测试 *产生*的文件：`./temp/test/`
    - 子路径模式：`{module_path}/{ok|err}/{test_fn_name}/` —— 模块路径必须完整且一致，从根模块开始，子模块用 `/` 连接（与
      Rust 模块路径一致，如 `wb_files_pack/manager_sync`）。模块路径和测试函数名必须与代码中写的一致。
    - `ok/` 存放预期成功的测试；`err/` 存放预期失败的测试。分类遵循测试的主导意图：当且仅当测试的核心断言是错误断言——对错误变体
      `matches!`、`catch_unwind` + `assert!(r.is_err())`、或 `should_panic`——时才归入 `err/`
      。按主导意图判定，而非按测试内部是否出现错误步骤。成功与错误权重相当且不可分割的混合测试必须按最小单位规则拆分为一个
      `ok/` 和一个 `err/` 测试。
- **测试资源只读**：
    - 测试绝不能修改 `resources/test/` 下的文件。
    - 如果所需资源不存在，在任何测试函数体执行前生成——首选在测试构建的编译时，否则在早于测试函数运行的 `#[cfg(test)]`
      setup 阶段。生成逻辑是测试专用代码，但绝不属于测试函数体；当测试函数体开始时，其资源必须已存在且只读。
    - 在测试时运行的生成必须用显式注释分界包裹：`// ===== TEST-ONLY RESOURCE GENERATION: START =====` ...
      `// ===== TEST-ONLY RESOURCE GENERATION: END =====`，且分界必须放在使用该资源的 Rust 测试函数内部——即生成逻辑的调用方——绝不放
      helper/生成函数自身内部。如果生成无法自动化，提供带显式注释的专门测试资源生成函数；生成必须在相应测试代码执行前发生。
    - 如果资源已存在，原样使用；不要在每次测试运行时重新生成。
    - 如果资源内容需要修改，先复制到 `temp/test/` 再修改副本——绝不修改资源本身。
- **临时目录生命周期**：
    - 测试开始前，始终尝试删除其临时文件；路径不存在不算错误，但任何其他非预期错误必须以失败终止测试。然后创建测试的临时目录。
    - 测试结束时，仅在测试通过时自动删除临时文件。如果测试期间发生非预期错误，保留临时文件作为证据（不删除）。
    - 如果结束时的清理本身遇到非预期错误（按代码逻辑判定），测试失败。

### 测试资源策略

- **双层资源策略**：
    - 固定、确定性资源：位于 `resources/test/`，原样使用且只读。跨运行复用——非随机化测试不会每次运行重新生成资源文件；仅缺失时生成。
    - 动态、每次运行产生的产物：位于 `temp/test/`，遵循上述标准的 `{module_path}/{ok|err}/{test_fn_name}/`
      布局，按上述临时目录生命周期创建与清理。
- **随机化测试**：
    - 随机化测试绝不能生成资源文件——只能产生 `temp/test/` 下的临时文件（随机夹具是测试产物，绝不是资源）。
    - 发生非预期错误时，临时文件保留（不删除）；失败现场遗留的文件本身就是复现手段——检查它们即可诊断。无需种子重放机制。
    - 随机化测试仅在全量模式（`cargo test -- --include-ignored`）运行，用 `#[ignore = "longtime"]` 门控。

### 测试意图与错误断言

- **测试范围限定到最小单位**：每个测试必须只覆盖一个行为且设置最少。不要把无关断言或多模块流程塞进一个测试——如果一个测试需要
  N 个独立行为，拆成 N 个测试。
- **断言预期结果，而非"通过"**：测试的目的是验证具体预期结果。钉死精确的预期值/状态（字节内容、长度、空错误列表、精确错误变体）。只要预期值可知，
  `is_ok()` 这类宽泛断言单独使用是不够的。
- **故障注入测试必须限定错误范围**：当测试故意触发错误时，必须验证恰好发生了预期错误且无其他错误；任何其他错误（错误变体、来源、载荷不对）都是失败。优先类型级匹配——错误枚举变体、
  `PoisonError`、`downcast_ref` 载荷——而非粗粒度 `is_err()`。
- **绝不断言外部来源错误的消息文本**：OS/标准库/第三方错误消息因平台、locale 和版本而异——不要断言其 `Display`/
  `to_string()` 文本。改为匹配错误 kind/type/variant。例外：测试自身注入的消息（如 `panic!("literal")` 载荷）完全受控，可以精确匹配。

## 错误处理规范

- `water_ball_tool_lib/src/lib.rs` 全局 deny `clippy::unwrap_used`，且生产（非 test）构建额外 deny
  `clippy::expect_used`（`#![cfg_attr(not(test), deny(clippy::expect_used))]`——测试目标仍保留 warn，直到既有测试中的
  expect
  完成迁移）。二进制 crate `src/main.rs` 同样 deny `clippy::unwrap_used`。绝不抑制这些 lint。绝不在库或应用代码中使用
  `.unwrap()`、`.expect()` 或任何 panic 式 unwrap。
- **可恢复错误必须通过 `?` 操作符传播**给调用方，使用模块的错误类型（`wb_files_pack` 中为 `PackFileError`，`file_finder` 中为
  `io::Error` 或 `SearchResult`/`SearchWarning`）。
- **不应中止执行的非致命错误（警告）**：
    - 同步 API（`FileFinder::search`）：收集到 `SearchResult.warnings: Vec<SearchWarning>`。
    - 流式 API（`FileFinder::search_stream`）：通过接收通道发出 `SearchEvent::Warning(SearchWarning)`。
- **Mutex 毒化**：使用 `.lock().unwrap_or_else(std::sync::PoisonError::into_inner)` 恢复锁——避免毒化时 panic。
- **线程 join**：使用 `handle.join().map_err(|_| ...)?` 将 panic 作为错误传播，而不是调用 `.unwrap()`。
- **创建线程失败必须返回错误**：用 `thread::Builder::new()...spawn(...)` 创建线程，并用 `?` 传播失败（必要时映射为模块错误类型）。
  `std::thread::spawn` 在操作系统无法创建线程时会 panic， **禁止使用**。绝不允许只记录 spawn 失败后继续当作线程已存在。
- **逻辑错误必须使用 `unreachable!`**：在正常逻辑下永远无法到达的代码属于逻辑错误——一旦到达意味着不变量被破坏。此类分支必须用
  `unreachable!("<原因>")` 中止；不要用 `panic!`，也不要记录日志后继续。反之，运行期确实可能发生的失败（I/O 错误、锁毒化、非法用户
  输入、spawn 失败） **不是**逻辑错误：必须返回或报告，绝不能改写成 `unreachable!`。
- **非 panic 辅助函数**：`crate::command::progress_style(template)` 构造 `ProgressStyle`；模板非法时记录错误并返回默认样式，不会
  panic。
- **禁止忽略警告的属性（FORBIDDEN）**：`#[allow(...)]`、`#![allow(...)]`、`#[expect(...)]` 及任何其他 lint
  抑制属性不得添加到生产或测试代码中。不要掩盖警告——而是修复根本原因（重构代码、传播错误，或在测试中使用不 panic 的辅助函数如
  `TestTool::expect_ok`）。

## 架构说明（从文件布局不易看出）

### workspace 与 crate 布局

- 根 `Cargo.toml` 是 **workspace**（`resolver = "3"`），成员为 `water_ball_tool_lib`；同一文件还声明了源码位于 `src/` 的
  `water_ball_tool` 二进制包。
- `water_ball_tool_lib/src/lib.rs` 导出库模块：`file_finder`、`wb_files_pack`、`tools`、`concurrent_ring`、
  `split_ring_buffer`，以及 `gakumasu`（`#[cfg(debug_assertions)]` 门控——仅 debug 构建编译）。
- `src/main.rs` 声明 `mod command;` —— `command.rs` 对二进制 crate 私有， **不是**库的一部分。
- 二进制代码通过 crate 名限定路径引用库——`water_ball_tool_lib::file_finder`、`water_ball_tool_lib::wb_files_pack`、
  `water_ball_tool_lib::tools`、`water_ball_tool_lib::concurrent_ring`、`water_ball_tool_lib::split_ring_buffer`——而非
  `crate::`。
- `command/ff.rs` — `ff` 子命令处理：参数解析、进度条设置、文件搜索编排与可选哈希。调用库中的
  `FileFinder::search_stream()`。命令层工作/哈希线程用 `thread::Builder` 创建，spawn 失败作为错误传播。
    - `[OUT_PATH]` 之后的可选多值 **位置参数** `[HASH]...`：`all`、`md5`、`sha256`、`sha512`、`sha1`、`blake3`、
      `blake2b`、`crc32`（别名 `*`、`m5`、`s256`、`s512`、`s1`、`b3`、`b2b`、`crc`）。哈希会附加到每个文件条目，并出现在
      JSON 输出中。
    - 开启哈希时文件发现被强制为单线程（`ff_thread_count = 1`），同时由独立的哈希工作线程池（数量为 `thread_count`，取自
      `-t/--thread-count`，缺省为可用并行度）消费共享环形缓冲区。
- `command/wbfp.rs` — `wbfp` 子命令处理：打包/解包/哈希校验，多线程文件 I/O，每工作线程独立进度条，通过
  `FileFinder::search_stream()` 流式发现文件。打包/解包/校验工作线程均用 `thread::Builder` 创建，spawn 失败作为错误传播。
- `command/music_play.rs` + `command/music_play/` — `mp` 子命令处理（模块 `music_play`，枚举变体 `MP`），终端音乐播放器，见下文。
- `command/test.rs` — `ff` 和 `wbfp` 命令的集成测试，含跨平台符号链接循环检测测试。
    - 长时间测试（`#[ignore = "longtime"]`）使用 `create_large_fixture()` 生成 1,000 个随机文件（25 目录 × 40 文件，经
      `rand` crate 随机内容/大小）做压力测试。这些取代了旧的系统目录扫描测试。

### 文件搜索器 API（`water_ball_tool_lib/src/file_finder.rs`）

- `FileFinder::search(path, skip_symlinks, pb_tx, max_threads)` — 阻塞式多线程搜索，返回 `FilesList` 树。
- `FileFinder::search_stream(path, skip_symlinks, pb_tx, max_threads)` — 非阻塞流式搜索；返回
  `(JoinHandle<io::Result<()>>, Receiver<(PathBuf, FileInfo)>)`。工作线程传 `None` 结果以跳过 HashMap 收集开销。供 `wbfp`
  打包与 `ff` 哈希的生产者-消费者模式使用。
- 符号链接循环检测使用每线程 inode 链：每个工作线程维护一个经由符号链接进入的目录的 `Vec<InodeKey>`。遇到 symlink→dir 时，目标
  inode 与链比对。
- Inode 键：Unix 上 `(dev << 64) | ino`；Windows 上 `canonicalize()`。
- 限频进度：每 50 个文件或 10 个目录更新一次。
- 树重建使用 **迭代式基于栈的后序遍历**（Pre/Post 状态机）而非递归——避免深层嵌套目录栈溢出。父节点查找用 `rposition()`
  向后搜索以正确匹配路径。
- 测试文件：`water_ball_tool_lib/src/file_finder/test.rs` — 覆盖符号链接发现、祖先循环检测、多层符号链接、跳过标志、断链、混合
  场景、链传播、`search_stream` 正确性。

### 环形缓冲区 API

- `concurrent_ring`（`water_ball_tool_lib/src/concurrent_ring.rs`）— 可并发初始化的环形缓冲区（`ConcurrentRing<T>`），用作搜索/解码
  线程与工作线程之间的生产者-消费者队列。
- `split_ring_buffer`（`water_ball_tool_lib/src/split_ring_buffer.rs`）— 读写分离环形缓冲区，读端与写端可各自独立选择有锁或无锁
  （`locked_ring_buffer`、`unlocked_read_locked_write_ring_buffer` 等）；音频回调用的是无锁读端。

### 工具 API（`water_ball_tool_lib/src/tools.rs`）

- `PathTool::path_to_string_vec(path)` — 将路径拆分为字符串片段。
- `PathTool::path_remove_head(path, head)` — 去除 head 前缀，返回相对路径。
- `bytes_len_to_string(len)` — 将字节数格式化为可读形式（B/KiB/MiB/GiB）。
- `TestTool::prepare_test_dir(dir)` — 删除任何残留测试临时目录（容忍 `NotFound`，其他错误 panic）并重建。测试开始时调用。
- `TestTool::cleanup_test_dir(dir)` — 测试结束时删除测试临时目录；仅测试通过时执行（失败的测试在其前 panic，保留证据）。清理出错
  panic，测试失败。
- `TestTool::remove_test_pack_files(path)` — 测试中清理 `.pack`/`.wbm`/`.lock` 文件的辅助函数。

### WBFP 公共 API

- 公共入口点是 `ManagerSync`（`water_ball_tool_lib/src/wb_files_pack/manager_sync.rs`），而非 `WBFPManager`。
- `ManagerSync` 在 `Arc<Mutex<>>` 后包装 `WBFPManager` + `PackIO` 以实现线程安全访问。
- `ManagerSync::open(path)` — 以只读模式打开已有包文件。
- `ManagerSync::create_new(path)` — 以默认配置（分离清单、无 COW）创建新包文件。文件已存在则失败。
- `ManagerSync::options() -> PackOpenOptions` — 完整控制的构造器模式：`read()`、`write()`、`create()`、`create_new()`、
  `cow()`、`separate_manifest()`。
- `ManagerSync::open_virtual_file(path, end_pos)` — 以只读模式打开已有虚拟文件。
- `ManagerSync::create_virtual_file(path)` — 以只写模式创建新虚拟文件。
- `ManagerSync::virtual_file_options() -> VirtualFileOpenOptions` — 虚拟文件访问构造器：`read()`、`write()`、
  `create_new()`、`end_pos()`、`alloc_size()`。`alloc_size(Some(len))` 以 128B 对齐预分配已知文件大小；`None`
  （默认）将未知大小文件整块分配为 4MiB 块；打开已有文件时忽略。
- `PackVirtualFile`（由 `PackFileWR` 更名）— 实现 `Read`/`Write`/`Seek` 并带访问模式强制的虚拟文件句柄。
    - `get_len()` 返回 `u64`（非 `Result<u64>`）；`get_modified()` 返回 `u128`（非 `Result<u128>`）。
- 删除/擦除 API：`delete_file`、`delete_dir_all`、`erase_file(strategy)`、`erase_dir_all(strategy)`。
    - 删除：移除元数据 + 结构，数据块提交 GC（不覆写）。
    - 擦除：先向存储覆写数据块 + 清单块，然后 GC + 移除。
  - `OverwriteStrategy` 枚举：`Zero`、`Random`（rand crate）、`Dod5220`（3 遍：0x00→0xFF→随机——每遍写入磁盘）。数据块按块
    逐段覆写，每种策略/每一遍的写偏移都会随已写长度推进。
    - 根保护：空路径列表拒绝。`PackStruct` 上的 `child_locked_count` 阻止删除有锁定子项的目录。

### 音乐播放器（`mp`）

- `command/music_play.rs` — `mp` 子命令（模块 `music_play`，枚举变体 `MP`）：从文件/目录收集曲目（用 `lofty` 读取元数据），
  构建 `PlayList`，并通过 stdin 命令循环（`next`/`n`、`Play`/`P`、`pause`/`p`、`exit`）驱动 `Runtime`。
- 参数：位置参数 `files...`；`-v/--volume <f32>` 初始音量（0.0–1.0 小数，默认 `0.75`；超出范围时在开始播放前以参数错误返回失败）；
  `--verbose`（仅长选项）；`-m/--mode`，取值 `sequential-loop`（默认）、`sequential`、`repeat-one`、`shuffle`、
  `periodic-shuffle`。
- `command/music_play/` 下的模块：
    - `player.rs` — cpal 输出流、transport 命令（`PlayCmd`）、音量/位置原子量、`PlayerEvent` sink、`Player::try_stop()`。
    - `decoder_runtime.rs` — symphonia 解码线程，向前效果器环形缓冲区喂数据。格式读取器/解码器与轨道元数据合并保存在同一个
      `DecoderSession` 中，由单个 `Arc<Mutex<Option<...>>>` 保护，成对替换/取出。`run()` 在线程无法创建时返回错误；解码循环
      上报真实运行期错误并停止，单帧 `DecodeError` 跳过继续，停止/切曲时的 `WriteRejected` 属于正常控制流。
    - `effect.rs` + `effect/runtime.rs` — 效果器 trait 与带锁的前效果器运行时线程；具体效果器为 `effect/resampler.rs`、
      `effect/cross_bridge.rs`、`effect/fade.rs`。
    - `play_list.rs` — 曲目列表与循环/洗牌逻辑（`sequential-loop`、`sequential`、`repeat-one`、`shuffle`、
      `periodic-shuffle`）。
    - `runtime.rs` — 播放编排：UI 刷新线程、自动换曲、事件消费。
    - `event_dispatch.rs` — 面向实时音频回调的专用事件/日志分发线程。
    - `tools.rs` — `TimeMetrics` / `DurationAccumulator` 性能统计辅助。
    - `tui/`（`app.rs`、`ui.rs`、`channel_writer.rs`）— **开发中**，尚未接入命令。
- **实时线程规则**：cpal 回调绝不能加锁、调用 sink 或写日志。它只用非阻塞 `try_send` 把事件和日志记录投递到有界通道（容量
  1024；通道满
  则丢弃并计数）。`player-event-dispatch` 线程负责加锁、调用 `PlayerEventSink::emit` 以及真正输出 `tracing` 日志。
- 曲目结束通过原子 EOF 标志检测；回调只投递 `TrackEnded` 事件，不直接调用 sink。
- **退出顺序（`Player::try_stop`，幂等）**：暂停音频流 → 停止前效果器运行时（唤醒阻塞在 `push_wait` 的生产者）→ 停止解码线程 →
  停止事件分发线程 → join `rx_cmd` 线程。

### 日志 + 进度条

- 每个子命令处理函数调用 `crate::multi_progress_init(module)`（`module` 为 `"ff"`、`"wbfp"` 或 `"music_play"`）。它创建该命令的
  `MultiProgress`，并通过 `init_global_logging(&mp, module) -> Option<PathBuf>` 安装全局 tracing subscriber（若创建了日志文件则
  返回其路径）。
- 终端层包装 `MultiProgress`（每行输出周围调用 `suspend()`），日志不会撕裂进度条。
- Debug 构建（`#[cfg(debug_assertions)]`）：仅终端输出，级别 `debug`。Release 构建：终端级别 `info`，另外写级别 `debug` 的日志
  文件，采用方括号格式 `[时间戳] [级别] [线程] [文件:行] 消息`。
- 日志文件路径：`<ProjectDirs data_local_dir>/<crate>/logs/<module>/<module>-<YYYY-MM-DD_HH-MM-SS>.log`。若无法创建文件，回退为
  仅终端的 `debug` 级别输出。
- `main.rs` 本身只负责解析 `Cli` 并分发；进度/日志初始化在每个子命令内完成。

### 进度条辅助函数（`command.rs`）

- `create_pb(mp)` — 在 `MultiProgress` 中创建并注册自动旋转的 spinner，返回 `ProgressBar`。
- `progress_style(template) -> ProgressStyle` — 根据模板字符串构造样式；模板非法时记录错误并回退默认进度条样式（不
  panic）。应使用它
  替代 `ProgressStyle::tem*().expect(...)`。
- 常量：`BUF_LEN` = 10 MiB（读写缓冲区）、`PACK_PROGRESS_STYLE_TEMPLATE`（打包/解包/哈希：前缀、进度条、耗时/ETA、百分比、
  字节）、`SPINNER_TEMPLATE`（空闲/挂起的工作线程 spinner）。

### 代码变更标记

- `//AI===` / `//AI_END===` 是代码变更区域标记。
- 每个修改、新增或删除的代码段必须以成对标记包裹（`//AI===` 起始、`//AI_END===` 结束）。
- 起始标记可附一句功能简述。
- 标记只包裹代码，不包裹文档：禁止在 `///` / `//!` 文档注释内部或两侧放置标记（文档行之间插入普通 `//`
  注释会静默丢弃其后的文档内容）。仅修改文档注释时不需要任何标记；代码与其文档注释一起修改时，只标记代码。

### gakumasu

- `water_ball_tool_lib/src/gakumasu/` 下的开发中游戏模拟器。尚未接入 CLI。数据类型使用中文命名。

## WBFP 二进制格式速查

- 文件扩展名：`.pack`（包文件）、`.wbm`（分离清单）、`.lock`（基于 PID 的写锁）。
- 清单数据块使用 A/B 双块原子写入，BLAKE3 保证完整性。
- 渐进式保存：每写入 64MiB 或新增 10,000 个文件自动保存。
- 垃圾回收：批量排序（`sort_unstable_by_key`）+ 单遍合并相邻空闲块。O ((K+M) log (K+M))。
- 完整规范：`docs/zh_CN/wb_files_pack/manifest-data.md`

## 文档同步

- 每个编码任务必须更新所有相关文档文件以反映变更。包括：
    - **项目根目录管理/架构文档**：`AGENTS.md`、`README.md` 及仓库根目录的任何其他 Markdown 文件。
    - **技术规范文档**：`docs/en_US/*.md` 与 `docs/zh_CN/*.md` — 格式规范、架构决策、设计文档。
    - **代码级文档**：模块级文档注释（`//!`）、公共 API 文档注释（`///`）、内联注释。
- 过时文档视为缺陷。在所有受影响文档更新前，实现不算完成。
- 同步范围包括但不限于：新公共 API、变更签名、移除的方法、新模块、变更行为、更新约定。
- **多语言同步强制**：每份文档都有语言版本——英文（`docs/en_US/` 与仅英文的根目录文件）与中文（`docs/zh_CN/`
  ）。任何文档被修改时，其全部语言版本必须在同一变更中同步更新，内容与结构保持一致；在所有语言版本反映该更新前，文档更新不算完成。
- 每份文档必须保持其语言切换链接（`> **Language**: English | [简体中文](...)` / `> **语言**: [English](../...) | 简体中文`
  ）指向正确的对应版本。新增或移除文档时，必须在每个语言版本中更新指向它的所有交叉引用与语言链接。

## 规范文件语言

- 规范文件（如本文件 `AGENTS.md`）必须仅使用英文——不得混杂其他语言。代码字面量（标识符、命令名、路径字符串、错误/panic 消息）豁免。
- 根目录规范文件必须保持仅英文；其中文翻译位于 `docs/zh_CN/AGENTS.md`，作为按上述多语言同步规则保持同步的参考副本。

## 双语注释

- 注释为中文 + 英文。中文为主，英文通常是翻译而非独立文档。添加注释时遵循现有双语模式。
