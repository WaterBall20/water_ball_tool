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
cargo test --workspace         # 跳过长时间测试（#[ignore] 自动处理）
cargo test --workspace -- --include-ignored # 包含长时间测试（主分支推送时全量 CI）
./target/release/water_ball_tool -h
```

- `wbfp` 在 stdout/stdin 为终端时运行 TUI；`--no-tui`（或非 TTY stdout）保留经典进度条输出。`wbfp browse <pack>` 只打开只读
  浏览器，不启动任务。打包/解包/校验期间 TUI 文件列表仅支持预览。

## 构建选项

- 仓库是一个 **workspace**：根 `Cargo.toml` 定义 `water_ball_tool` 二进制包，并把 `water_ball_tool_lib` 列为成员。
  在仓库根执行 `cargo build` 会同时构建两者。
- `.cargo/native-config.toml` **不会**自动生效（cargo 只自动加载名为 `config.toml` 的文件）。需要显式传入：
  `cargo --config .cargo/native-config.toml build --release`。它使用 `-C target-cpu=native` 编译，并把产物重定向到
  `target/native/`（二进制位于 `target/native/release/water_ball_tool`）。不要把 native 构建当作可分发版本。
- 仓库通过 `rust-toolchain.toml` 固定 **nightly**（`channel = "nightly"`），因此此仓库中的所有 `cargo` 命令都使用
  nightly。`build_native.ps1` 在 `.cargo/native-config.toml` 基础上使用 `-Zbuild-std="std,panic_abort"` 执行本机优化
  release 构建。
- `debug-no-audio` feature 是 `mp` 命令的调试辅助：cpal 回调会跳过环形缓冲区读取、后置效果器处理和音量缩放，便于在没有真实
  音频的情况下快速调试播放控制。它不产生可用的音频输出。该 feature 定义在 `water_ball_tool_lib`（cpal 回调位于库包），
  二进制包负责转发，因此从仓库根执行 `cargo build --features debug-no-audio` 仍然有效。

## 测试规范

- 慢速测试标记为 `#[ignore = "longtime"]`。`cargo test --workspace` 默认跳过它们。主分支 CI 推送时使用 `--include-ignored`
  运行全量套件。必须带 `--workspace`：`water_ball_tool_lib` 是 workspace 成员，单独的 `cargo test` 只覆盖二进制包。
- `ff` 与经典 `wbfp` 路径通过 `crate::multi_progress_init(module)` 自行初始化日志，因此调用其处理函数的测试无需额外设置
  即可获得可用的 `MultiProgress`。`wbfp` TUI 与 `mp` 安装 TUI 日志通道（`tui::channel_writer::init_logging`），因此测试
  不应调用 `wbfp::run`/`music_play::args` 并期待终端。重复注册 subscriber 会被 `try_init` 拒绝并仅记录日志。测试使用
  `wbfp::commands`，它始终走经典路径。
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
    - 随机化测试仅在全量模式（`cargo test --workspace -- --include-ignored`）运行，用 `#[ignore = "longtime"]` 门控。

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

## 架构

模块级设计位于 [ARCHITECTURE.md](ARCHITECTURE.md)：改动哪个模块就读对应章节——`ff` §6、`mp` §7、`wbfp` §8、
进度条 §9、日志 §10、工具 §11、环形缓冲区 §12——不要在本文件中重复。二进制格式规范位于 `docs/zh_CN/`
（英文：`docs/en_US/`）。

任何改动都必须遵守、且模块文档不会重复的规则：

- `src/command.rs` 对二进制 crate 私有；二进制代码通过 `water_ball_tool_lib::<module>` 引用库。
- 实时音频回调绝不加锁、不调用 sink、不写日志；`Player::try_stop` 的关停顺序固定（ARCHITECTURE §7.3、§7.5）。
- 模块细节写入 ARCHITECTURE.md 或模块的 `//!` 文档——不要写进本规范文件。

## 代码变更标记

- `//AI===` / `//AI_END===` 是代码变更区域标记。
- 每个修改、新增或删除的代码段必须以成对标记包裹（`//AI===` 起始、`//AI_END===` 结束）。
- 起始标记可附一句功能简述。
- 标记只包裹代码，不包裹文档：禁止在 `///` / `//!` 文档注释内部或两侧放置标记（文档行之间插入普通 `//`
  注释会静默丢弃其后的文档内容）。仅修改文档注释时不需要任何标记；代码与其文档注释一起修改时，只标记代码。

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
