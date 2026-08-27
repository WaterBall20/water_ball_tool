# AGENTS.md — 水球工具

> **语言**: [English](../AGENTS.md) | 简体中文

Rust 2024 CLI 工具箱。两个命令：`ff`（并行文件搜索器）和 `wbfp`（自定义二进制打包格式）。

## 命令

```bash
cargo build --release           # 发布构建
cargo check                     # 仅编译检查（快速）
cargo clippy                    # 代码检查
cargo test                      # 跳过长时间测试（#[ignore] 自动处理）
cargo test -- --include-ignored # 包含长时间测试（主分支推送时全量 CI）
./target/release/water_ball_tool -h
```

## 测试规范

- 慢速测试标记为 `#[ignore = "longtime"]`。`cargo test` 默认跳过它们。主分支 CI 推送时使用 `--include-ignored` 运行全量套件。
- 使用 `indicatif::MultiProgress` 的测试必须首先调用 `crate::init_global_logging(&mp)`——否则因 tracing subscriber 未初始化而日志 panic。
- 测试临时目录位于 `./temp/test/`（gitignore）。
- 测试开始用 `TestTool::prepare_test_dir(dir)`、测试结束用 `TestTool::cleanup_test_dir(dir)`（均来自 `tools.rs`；两者容忍 `NotFound`；清理遇到非预期错误会 panic）。`TestTool::remove_test_pack_files(path)` 在打包测试后删除 `.pack`/`.wbm`/`.lock` 文件。
- 跨平台测试使用 `#[cfg(unix)]` / `#[cfg(windows)]` 创建符号链接及平台相关路径。

### 测试目录与资源生命周期

- **测试绝不能互相依赖**：每个测试完全自包含；其消费和产生的目录与所有其他测试的目录互不相交。
- **资源目录布局**：
  - 测试夹具——测试*需要*的文件：`./resources/test/`
  - 测试输出——测试*产生*的文件：`./temp/test/`
  - 子路径模式：`{module_path}/{ok|err}/{test_fn_name}/` —— 模块路径必须完整且一致，从根模块开始，子模块用 `/` 连接（与 Rust 模块路径一致，如 `wb_files_pack/manager_sync`）。模块路径和测试函数名必须与代码中写的一致。
  - `ok/` 存放预期成功的测试；`err/` 存放预期失败的测试。分类遵循测试的主导意图：当且仅当测试的核心断言是错误断言——对错误变体 `matches!`、`catch_unwind` + `assert!(r.is_err())`、或 `should_panic`——时才归入 `err/`。按主导意图判定，而非按测试内部是否出现错误步骤。成功与错误权重相当且不可分割的混合测试必须按最小单位规则拆分为一个 `ok/` 和一个 `err/` 测试。
- **测试资源只读**：
  - 测试绝不能修改 `resources/test/` 下的文件。
  - 如果所需资源不存在，在任何测试函数体执行前生成——首选在测试构建的编译时，否则在早于测试函数运行的 `#[cfg(test)]` setup 阶段。生成逻辑是测试专用代码，但绝不属于测试函数体；当测试函数体开始时，其资源必须已存在且只读。
  - 在测试时运行的生成必须用显式注释分界包裹：`// ===== TEST-ONLY RESOURCE GENERATION: START =====` ... `// ===== TEST-ONLY RESOURCE GENERATION: END =====`，且分界必须放在使用该资源的 Rust 测试函数内部——即生成逻辑的调用方——绝不放 helper/生成函数自身内部。如果生成无法自动化，提供带显式注释的专门测试资源生成函数；生成必须在相应测试代码执行前发生。
  - 如果资源已存在，原样使用；不要在每次测试运行时重新生成。
  - 如果资源内容需要修改，先复制到 `temp/test/` 再修改副本——绝不修改资源本身。
- **临时目录生命周期**：
  - 测试开始前，始终尝试删除其临时文件；路径不存在不算错误，但任何其他非预期错误必须以失败终止测试。然后创建测试的临时目录。
  - 测试结束时，仅在测试通过时自动删除临时文件。如果测试期间发生非预期错误，保留临时文件作为证据（不删除）。
  - 如果结束时的清理本身遇到非预期错误（按代码逻辑判定），测试失败。

### 测试资源策略

- **双层资源策略**：
  - 固定、确定性资源：位于 `resources/test/`，原样使用且只读。跨运行复用——非随机化测试不会每次运行重新生成资源文件；仅缺失时生成。
  - 动态、每次运行产生的产物：位于 `temp/test/`，遵循上述标准的 `{module_path}/{ok|err}/{test_fn_name}/` 布局，按上述临时目录生命周期创建与清理。
- **随机化测试**：
  - 随机化测试绝不能生成资源文件——只能产生 `temp/test/` 下的临时文件（随机夹具是测试产物，绝不是资源）。
  - 发生非预期错误时，临时文件保留（不删除）；失败现场遗留的文件本身就是复现手段——检查它们即可诊断。无需种子重放机制。
  - 随机化测试仅在全量模式（`cargo test -- --include-ignored`）运行，用 `#[ignore = "longtime"]` 门控。

### 测试意图与错误断言

- **测试范围限定到最小单位**：每个测试必须只覆盖一个行为且设置最少。不要把无关断言或多模块流程塞进一个测试——如果一个测试需要 N 个独立行为，拆成 N 个测试。
- **断言预期结果，而非"通过"**：测试的目的是验证具体预期结果。钉死精确的预期值/状态（字节内容、长度、空错误列表、精确错误变体）。只要预期值可知，`is_ok()` 这类宽泛断言单独使用是不够的。
- **故障注入测试必须限定错误范围**：当测试故意触发错误时，必须验证恰好发生了预期错误且无其他错误；任何其他错误（错误变体、来源、载荷不对）都是失败。优先类型级匹配——错误枚举变体、`PoisonError`、`downcast_ref` 载荷——而非粗粒度 `is_err()`。
- **绝不断言外部来源错误的消息文本**：OS/标准库/第三方错误消息因平台、locale 和版本而异——不要断言其 `Display`/`to_string()` 文本。改为匹配错误 kind/type/variant。例外：测试自身注入的消息（如 `panic!("literal")` 载荷）完全受控，可以精确匹配。

## 错误处理规范

- `src/lib.rs` 全局 deny `clippy::unwrap_used`。绝不抑制它。绝不在库或应用代码中使用 `.unwrap()`、`.expect()` 或任何 panic 式 unwrap。
- **可恢复错误必须通过 `?` 操作符传播**给调用方，使用模块的错误类型（`wb_files_pack` 中为 `PackFileError`，`file_finder` 中为 `io::Error` 或 `SearchResult`/`SearchWarning`）。
- **不应中止执行的非致命错误（警告）**：
  - 同步 API（`FileFinder::search`）：收集到 `SearchResult.warnings: Vec<SearchWarning>`。
  - 流式 API（`FileFinder::search_stream`）：通过接收通道发出 `SearchEvent::Warning(SearchWarning)`。
- **Mutex 毒化**：使用 `.lock().unwrap_or_else(std::sync::PoisonError::into_inner)` 恢复锁——避免毒化时 panic。
- **线程 join**：使用 `handle.join().map_err(|_| ...)?` 将 panic 作为错误传播，而不是调用 `.unwrap()`。
- **禁止忽略警告的属性（FORBIDDEN）**：`#[allow(...)]`、`#![allow(...)]`、`#[expect(...)]` 及任何其他 lint 抑制属性不得添加到生产或测试代码中。不要掩盖警告——而是修复根本原因（重构代码、传播错误，或在测试中使用不 panic 的辅助函数如 `TestTool::expect_ok`）。

## 架构说明（从文件布局不易看出）

### 模块图
- `src/main.rs` 声明 `mod command;` —— **而非** `lib.rs`。`command.rs` 对二进制 crate 私有。
- `src/lib.rs` 导出库 crate：`file_finder`、`wb_files_pack`、`tools`、`gakumasu`（`#[cfg(debug_assertions)]` 门控——仅 debug 构建编译）。
- `command.rs` 使用 `water_ball_tool::file_finder` 和 `water_ball_tool::wb_files_pack`（crate 名限定路径），而非 `crate::`——因为 command 在二进制 crate 中。
- `command/ff.rs` — `ff` 子命令处理：参数解析、进度条设置、文件搜索编排。调用库中的 `FileFinder::search()`。
- `command/wbfp.rs` — `wbfp` 子命令处理：打包/解包/哈希校验，多线程文件 I/O，每工作线程独立进度条，通过 `FileFinder::search_stream()` 流式发现文件。
- `command/test.rs` — `ff` 和 `wbfp` 命令的集成测试，含跨平台符号链接循环检测测试。
  - 长时间测试（`#[ignore = "longtime"]`）使用 `create_large_fixture()` 生成 1,000 个随机文件（25 目录 × 40 文件，经 `rand` crate 随机内容/大小）做压力测试。这些取代了旧的系统目录扫描测试。

### 文件搜索器 API（`src/file_finder.rs`）
- `FileFinder::search(path, skip_symlinks, pb_tx, max_threads)` — 阻塞式多线程搜索，返回 `FilesList` 树。
- `FileFinder::search_stream(path, skip_symlinks, pb_tx, max_threads)` — 非阻塞流式搜索；返回 `(JoinHandle<io::Result<()>>, Receiver<(PathBuf, FileInfo)>)`。工作线程传 `None` 结果以跳过 HashMap 收集开销。供 `wbfp` 打包命令的生产者-消费者模式使用。
- 符号链接循环检测使用每线程 inode 链：每个工作线程维护一个经由符号链接进入的目录的 `Vec<InodeKey>`。遇到 symlink→dir 时，目标 inode 与链比对。
- Inode 键：Unix 上 `(dev << 64) | ino`；Windows 上 `canonicalize()`。
- 限频进度：每 50 个文件或 10 个目录更新一次。
- 树重建使用**迭代式基于栈的后序遍历**（Pre/Post 状态机）而非递归——避免深层嵌套目录栈溢出。父节点查找用 `rposition()` 向后搜索以正确匹配路径。
- 测试文件：`src/file_finder/test.rs` — 覆盖符号链接发现、祖先循环检测、多层符号链接、跳过标志、断链、混合场景、链传播、`search_stream` 正确性。

### 工具 API（`src/tools.rs`）
- `PathTool::path_to_string_vec(path)` — 将路径拆分为字符串片段。
- `PathTool::path_remove_head(path, head)` — 去除 head 前缀，返回相对路径。
- `bytes_len_to_string(len)` — 将字节数格式化为可读形式（B/KiB/MiB/GiB）。
- `TestTool::prepare_test_dir(dir)` — 删除任何残留测试临时目录（容忍 `NotFound`，其他错误 panic）并重建。测试开始时调用。
- `TestTool::cleanup_test_dir(dir)` — 测试结束时删除测试临时目录；仅测试通过时执行（失败的测试在其前 panic，保留证据）。清理出错 panic，测试失败。
- `TestTool::remove_test_pack_files(path)` — 测试中清理 `.pack`/`.wbm`/`.lock` 文件的辅助函数。

### WBFP 公共 API
- 公共入口点是 `ManagerSync`（`src/wb_files_pack/manager_sync.rs`），而非 `WBFPManager`。
- `ManagerSync` 在 `Arc<Mutex<>>` 后包装 `WBFPManager` + `PackIO` 以实现线程安全访问。
- `ManagerSync::open(path)` — 以只读模式打开已有包文件。
- `ManagerSync::create_new(path)` — 以默认配置（分离清单、无 COW）创建新包文件。文件已存在则失败。
- `ManagerSync::options() -> PackOpenOptions` — 完整控制的构造器模式：`read()`、`write()`、`create()`、`create_new()`、`cow()`、`separate_manifest()`。
- `ManagerSync::open_virtual_file(path, end_pos)` — 以只读模式打开已有虚拟文件。
- `ManagerSync::create_virtual_file(path)` — 以只写模式创建新虚拟文件。
- `ManagerSync::virtual_file_options() -> VirtualFileOpenOptions` — 虚拟文件访问构造器：`read()`、`write()`、`create_new()`、`end_pos()`、`alloc_size()`。`alloc_size(Some(len))` 以 128B 对齐预分配已知文件大小；`None`（默认）将未知大小文件整块分配为 4MiB 块；打开已有文件时忽略。
- `PackVirtualFile`（由 `PackFileWR` 更名）— 实现 `Read`/`Write`/`Seek` 并带访问模式强制的虚拟文件句柄。
  - `get_len()` 返回 `u64`（非 `Result<u64>`）；`get_modified()` 返回 `u128`（非 `Result<u128>`）。
- 删除/擦除 API：`delete_file`、`delete_dir_all`、`erase_file(strategy)`、`erase_dir_all(strategy)`。
  - 删除：移除元数据 + 结构，数据块提交 GC（不覆写）。
  - 擦除：先向存储覆写数据块 + 清单块，然后 GC + 移除。
  - `OverwriteStrategy` 枚举：`Zero`、`Random`（rand crate）、`Dod5220`（3 遍：0x00→0xFF→随机——每遍写入磁盘）。
  - 根保护：空路径列表拒绝。`PackStruct` 上的 `child_locked_count` 阻止删除有锁定子项的目录。

### 日志 + 进度条
- `main.rs` 创建 `MultiProgress` 并传给 `init_global_logging()`，后者通过 `MultiProgressWriter` 适配器路由 `tracing` 输出，使日志不覆盖进度条。
- Debug 模式：日志级别 `debug`。Release 模式：`info`。由 `#[cfg(debug_assertions)]` 控制。

### 进度条辅助函数（`command.rs`）
- `create_pb(mp)` — 在 `MultiProgress` 中创建并注册 spinner。无进度模式返回 `None`。
- `set_pb_style2(pb, data_len)` — 切换到确定性条模式（`=>-` 字符），用于已知总长度操作。
- `update_pb(cb, path1, path2, last_written, current_written)` — 限频进度回调（每 10×BUF_LEN 字节触发一次）。
- 模板：`PROGRESS_STYLE_TEMPLATE`（搜索）和 `PACK_PROGRESS_STYLE_TEMPLATE`（打包/解包，带前缀、百分比、字节）。

### 代码变更标记

- `//AI===` / `//AI_END===` 是代码变更区域标记。
- 每个修改、新增或删除的代码段必须以成对标记包裹（`//AI===` 起始、`//AI_END===` 结束）。
- 起始标记可附一句功能简述。

### gakumasu
- `src/gakumasu/` 下的开发中游戏模拟器。尚未接入 CLI。数据类型使用中文命名。

## WBFP 二进制格式速查

- 文件扩展名：`.pack`（包文件）、`.wbm`（分离清单）、`.lock`（基于 PID 的写锁）。
- 清单数据块使用 A/B 双块原子写入，BLAKE3 保证完整性。
- 渐进式保存：每写入 64MiB 或新增 10,000 个文件自动保存。
- 垃圾回收：批量排序（`sort_unstable_by_key`）+ 单遍合并相邻空闲块。O((K+M) log(K+M))。
- 完整规范：`docs/zh_CN/wb_files_pack/manifest-data.md`

## 文档同步

- 每个编码任务必须更新所有相关文档文件以反映变更。包括：
  - **项目根目录管理/架构文档**：`AGENTS.md`、`README.md` 及仓库根目录的任何其他 Markdown 文件。
  - **技术规范文档**：`docs/en_US/*.md` 与 `docs/zh_CN/*.md` — 格式规范、架构决策、设计文档。
  - **代码级文档**：模块级文档注释（`//!`）、公共 API 文档注释（`///`）、内联注释。
- 过时文档视为缺陷。在所有受影响文档更新前，实现不算完成。
- 同步范围包括但不限于：新公共 API、变更签名、移除的方法、新模块、变更行为、更新约定。
- **多语言同步强制**：每份文档都有语言版本——英文（`docs/en_US/` 与仅英文的根目录文件）与中文（`docs/zh_CN/`）。任何文档被修改时，其全部语言版本必须在同一变更中同步更新，内容与结构保持一致；在所有语言版本反映该更新前，文档更新不算完成。
- 每份文档必须保持其语言切换链接（`> **Language**: English | [简体中文](...)` / `> **语言**: [English](../...) | 简体中文`）指向正确的对应版本。新增或移除文档时，必须在每个语言版本中更新指向它的所有交叉引用与语言链接。

## 规范文件语言

- 规范文件（如本文件 `AGENTS.md`）必须仅使用英文——不得混杂其他语言。代码字面量（标识符、命令名、路径字符串、错误/panic 消息）豁免。
- 根目录规范文件必须保持仅英文；其中文翻译位于 `docs/zh_CN/AGENTS.md`，作为按上述多语言同步规则保持同步的参考副本。

## 双语注释

- 注释为中文 + 英文。中文为主，英文通常是翻译而非独立文档。添加注释时遵循现有双语模式。
