# 更新日志

> **语言**: [English](../CHANGELOG.md) | 简体中文

## [Unreleased] — 2026-11-01

### WBFP TUI

- `wbfp p`、`wbfp u`、`wbfp h` 在 stdout 为终端时默认运行 ratatui TUI。文件列表只读，逐行显示状态并有全局任务卡片，
  右侧窗口预留内容预览。`--no-tui` 保留经典 indicatif 输出；非 TTY stdout 自动回退。
- 新增 `wbfp browse <pack>`（别名 `b`），不启动任务即可只读浏览。目录通过 `ManagerSync` 懒加载；打包模式浏览本地源目录。
- 无子命令的 `wbfp` 现在打开文件管理器；包路径可选，且该形式仅支持 TUI：`--no-tui` 与非 TTY 输入会被拒绝。管理器布局见下方条目。
- TUI 使用独立任务线程与 `JobEvent` 通道；UI 另开只读 `ManagerSync` 句柄，避免清单读取与打包/解包/校验文件句柄争锁。
  任务运行中按 `q` 会在完成后退出。
- `wbfp.rs` 拆分为 `classic.rs`（行为不变的进度条路径）、`common.rs`（共享路径/复制/解包）、`events.rs`、`ops.rs`
  与 `tui/`（状态、渲染、目录懒加载）。
- 调试（日志）与帮助弹层互斥，并可用打开时的同一按键关闭（`F12`、`?`）；`Esc` 关闭当前窗口。`d` 不再打开调试弹层。
- 目录行聚合后代状态：子项未完成时父目录保持 `⟳`，子项失败/跳过时父目录显示红/黄。哈希校验任务卡片不再显示输出字段。
- `mp` 与 `wbfp` 现在共用 `command/tui_input.rs`：鼠标捕获、Windows VT 报文解析、原始 stdin 读取、双击状态、
  带边框列表命中与滚轮步长。`wbfp` 因此在 Windows 上也支持鼠标，而不是仅键盘；捕获失败时继续以键盘运行。VT 模式下
  两套 TUI 都直接读取原始 stdin 字节，修复了 crossterm 0.29 丢弃控制字符导致主界面收不到按键的问题。TUI 诊断日志
  （路由、按键、目录加载、任务开始/结束、每 GiB 进度）写入 `temp/logs/wbfp/` 下的 debug 日志。
- 打包不再发送目录事件，空源目录不会一直显示进行中图标。
- 预览窗在任务运行期间显示实时任务列表：当前文件在最前并带逐文件进度，已完成文件在下方。工作线程通过非阻塞通道入队
  限流进度；UI 只维护有界队列，不为任务列表读取文件，因此不会拖慢打包/解包/校验 IO。
- 文件列表与任务列表使用固定显示宽度列（`unicode-width`）、固定进度条/百分比/大小列和固定高度的任务摘要，
  小文件快速完成时界面字段不再跳动。大小拆成右对齐数字列与固定宽度单位列，单位 `B` 只占一格也不会让数字横移。
- 任务卡片与逐文件任务条改用与 `mp` 共享的八分块进度条（`█▉▊▋▌▍▎▏`），进度精确到 1/8 格；经典 indicatif 进度条
  使用同一组字符。
- 写入优化跳过（大小与修改时间均未变化）现在标记为 `Done` 而不是警告，打包汇总也不再把它计入 warning。TUI 打开已有
  包时会像经典路径一样记录写入优化提示。任务卡片的总进度条改为独占一行、标签放到下一行，居中文字不再在进度条上挖出
  空洞。
- 无子命令的 `wbfp` 现在是文件管理器：左侧本地文件系统、中间可选包、右侧预览。包路径可选，裸 `wbfp` 隐藏中间栏。
  空间不足时先折叠预览、再折叠本地栏，包视图始终保留。`Tab` 切换面板，`Enter` 打开探测到的本地包，`Ctrl+W` 关闭。
- 管理器中 `y`/`x` 跨命名空间复制/移动（两个列表都可见时直接执行；否则切换到对方列表，顶部显示操作与源路径并等待
  `v`）。`c`/`m` 在同一命名空间内复制/移动，源列表只读，`v` 确认。`Esc` 在返回上级前取消目标面板、待执行传输、
  重命名或删除确认。
- 复制/移动在后台 `JobSpec::Transfer` 任务中运行，线程数使用建议值；右侧像打包/解包/校验一样显示任务卡片与逐文件
  进度，包含包↔包与本地↔本地传输。
- `r` 原地重命名，输入框预填并全选；`Del` 经 `y/n` 确认后删除。目录不能复制/移动到自身子树（导航时阻止并标红）。
- 本地文件按 WBFilesPack 文件头探测（不依赖扩展名），合法包显示品红色 `◆`。新增 `probe_pack` 只读文件头，不获取
  `.lock`。

### WBFP 库

- 新增 `ManagerSync::rename(from, to)`，用于包内同目录改叶子名称。它只重写所在的 `PackStruct` 块——数据块、元数据位置与
  哈希均不变——并拒绝跨目录移动；移动将使用独立 API。文件自身或目录的任一后代文件存在打开的虚拟文件句柄时返回
  `PackFileError::Lock`。同时修复了直属文件被打开时 `child_locked_count` 未给直接父目录计数的问题。
- 新增 `ManagerSync::move_entry(from, to)`，用于跨目录移动并可选改名。目标父目录必须已存在；同目录调用被拒绝
  （请使用 `rename`）；目标缺失/已存在、移动到目录自身子树、打开句柄与只读模式都会被拒绝。它会更新源与目标两条
  祖先链的累计计数，并先写目标 `PackStruct`、再写源 `PackStruct`。

### 音乐播放器 TUI

- `mp` 现在运行真正的 TUI：新的 `Runtime` 持有播放器与播放列表，后台 `mp-scan` 线程把位置参数转成曲目，
  `tui::run` 每 50ms 渲染一次。按键：`n` 下一曲、`←`/`→` 每次 5 秒跳转（长按预览、松开提交）、空格 播放/暂停、`p` 暂停、
  `P` 播放、`+`/`-` 音量、`q`/`Esc`/`Ctrl+C` 退出。终端初始化与还原使用 `ratatui::try_init`/`try_restore`。
- 旧的 `indicatif` UI 线程、stdin 命令循环与 `--ui-preview` 开关已删除。自动换曲在解码线程上由
  `DecoderEventType::Eof` 触发，立即用 `set_decoder_path_no_clear` 切换：旧曲尾音继续从缓冲区播放，新曲排在其后，
  因此换曲没有额外延迟也不会截断。手动下一曲会清空缓冲。打开曲目失败按 skip / retry / pause 分类，连续失败上限 5 次。
- `Track` 新增 `album` 与 `audio_info`（由扫描器从 lofty 填充），`PlayList::current_index()` 提供当前索引供播放列表高亮。
- `-a/--autoplay` 在找到第一首后开始播放。不带该参数时第一首只加载并保持暂停；若尚未加载曲目，按下播放也会先加载第一首。
- 启动初始化按模式区分：顺序模式立即加载第一首；`shuffle` 与 `periodic-shuffle` 先等 5 首，扫描提前结束不足 5 首时
  用已有曲目兜底初始化。在此之前用户手动 `n`/播放会取消自动初始化。
- `F12` 切换 Debug 弹窗（左右布局，右侧显示实时日志）：前效果器/音频缓冲区的已用/容量、当前解码路径与格式、播放（设备）格式、
  播放列表状态、`lock_fail`/`underrun` 计数，以及解码/效果器/音频回调各段的 `TimeMetrics` 时间窗口。新增
  `DecoderRuntime::decoder_path()`
  暴露解码路径。
- 解码器在内部缓冲区累积交错样本，达到前效果器链的 `min_input_len_work()` 提示值后才写入数据块；该值只是批处理提示，
  更小的输入是否工作取决于效果器实现。
- Debug 弹窗新增基于缓冲区的延迟估算：解码器内部缓冲、解码→前效果排队、前效果延迟线、播放环加最近一次回调块，以及
  合计；以按比例绘制的管道条显示，左端为解码、右端为播放。暂不含设备/OS 延迟。
- 修复换曲后延迟估算持续累积的问题：清空前效果器输入环（skip/seek/重启）时同步清零排队样本计数，因为被丢弃的块不会
  再被效果线程取走。
- 新增效果器窗口（`e` 或 `:effects`）：左右分栏。左栏只列前/后效果器的名称与启用状态，右栏显示选中项所属链、延迟、介绍
  和设置占位；`↑`/`↓` 与 `PageUp`/`PageDown` 切换选择。列表只通过 `PreEffect`/`PostEffect` 的 trait 方法
  （`display_name`、`description`、`enabled`、`buffered_samples`）生成，新增效果器无需修改 UI；`空格`/`Enter` 通过 trait
  切换选中效果器的启用状态，其他设置编辑暂未实现。
- 效果器开关改为安全应用：前效果器开关进入独立命令通道，由效果器线程执行；`ConcurrentRing::wake_blocked_readers`
  用新的 `ReadWoken` 错误唤醒阻塞的 `pop_wait`，且不消费数据。禁用前效果器时通过
  `PreEffectDiscontinuity::DisableSelf` 排空内部缓冲，再把排出的数据交给下游的 `UpstreamDisabled { distance }`
  处理；只有有效格式变化才重置下游。重采样器会改变输出格式，静态标记为 `DisablePolicy::Forbidden`，不可禁用。
  后效果器严格同步，直接切换。`reset` 新增 `ResetReason`，效果器可区分新曲、格式变化与开关切换。UI 不再乐观更新；
  禁用的效果器延迟显示为 0，详情区把不可禁用项标记为固定。
- 分离环形缓冲区的 `write_all` 改为返回专用 `WriteAllError { written, kind }`，所有失败都携带已写入元素数。
  `wake_blocked_writers` 为非破坏性唤醒（`Interrupted`，可从 `written` 续写），`reject_pending_writers` 保持
  “清空 + 拒绝”（`Rejected`）。前效果器线程缓存未写完的数据，先应用开关命令，再按顺序续写缓存，因此控制唤醒不会
  截断音频块。
- 修复效果器窗口一直为空：前效果线程在写输出环之前释放效果链锁，因此暂停或输出环满时不会无限阻塞
  `effect_summaries()`。`process` 期间仍持有链锁，UI 保留最后一次成功快照。
- 收窄底栏：`←`/`→` 跳转提示移到进度条，`↑`/`↓` 滚动提示移到播放列表标题，音量移到进度行右侧的紧凑百分比条；
  正在播放标题不再显示音量。底栏保留 `e 效果` 按钮，Debug 改用 `F12`。
- 主进度条改用八分块字符（`█▉▊▋▌▍▎▏`），位置以 1/8 格为步进，不再整格跳动；时间/百分比标签放在进度条右侧，
  进度条始终保持连续。该共享辅助函数同时供 `wbfp` 任务卡片与任务条使用。
- 日志从主页面移入 `F12` Debug 窗口右栏；主页面右侧改为预留区。Debug、效果器与帮助窗口互斥，`Esc` 优先关闭当前窗口。
- TUI 状态按 `playback`、`playlist`、`scan`、`seek`、`footer`、`mouse`、`command`、`effects`、`debug` 分组；
  窗口布尔值合并为单个 `Window` 枚举。按键按当前窗口独占路由：命令输入优先、`Esc` 关闭窗口、`Window::None` 才进入主界面，
  打开窗口时按键不会落到主界面。
- 播放列表面板现在只渲染可见行并支持滚动：`↑`/`↓` 滚动一行、`PageUp`/`PageDown` 滚动一页、`Home`/`End` 跳到首尾、
  鼠标滚轮滚动三行、左键双击某行播放该曲（单击忽略以避免误触）。换曲时视口跟随当前曲目。新增 `PlayList::jump_to` 与
  `Runtime::play_at`
  用于按索引跳转，TUI 运行期间启用鼠标捕获。
- 修复 Windows/ConPTY 下移动鼠标导致 TUI 退出的问题。终端可能把鼠标报文作为裸 X10 负载字节交付，坐标字节（例如
  `q`）因此被当成退出键。Windows 现在打开 `ENABLE_VIRTUAL_TERMINAL_INPUT`、请求 SGR 鼠标上报，并用 `vtparse`
  （`VtInput`）解析 VT 字节流，把滚轮与左键映射到播放列表；移动、中键、右键与释放事件忽略。若无法启用 VT 输入，
  则禁用鼠标，键盘仍可正常使用。
- TUI 日志：`channel_writer` 安装 tracing subscriber，把格式化后的行经有界通道（容量 1024，满则 `try_send` 丢弃）
  送入面板；Debug 构建面板显示 `debug`，同时把 `debug` 日志文件写到项目 temp 目录（`temp/logs/<module>/`），
  TUI 崩溃后仍可取证；Release 构建面板显示 `info` 且继续把 `debug` 日志文件写到用户数据目录。panic 与 TUI 主循环
  错误会写入该文件，按键/鼠标事件以 `debug` 级别记录。`main.rs` 向该层
  暴露文件 writer 与日志过滤器；一次 `write` 现在会切出全部完整行，不再丢掉第一个换行之后的内容。
- 面板对日志级别分色：时间戳压暗，级别词 TRACE 暗灰、DEBUG 蓝、INFO 绿、WARN 黄、ERROR 红。级别只从格式化行的前
  40 字节解析，因此消息里出现的级别词不会改变颜色。
- `mp` TUI 新增 `:` 命令模式：底栏变为输入行（`Enter` 执行、`Esc` 取消、`Backspace` 编辑）。命令按唯一前缀匹配：
  `quit`、`play`、`pause`、`toggle`、`next`、`prev`、`seek <mm:ss|+N|-N>`、`vol <0-100|+N|-N>`、`mode <name>`、
  `goto <n>`、`debug` 与 `help`/`?`；未知或歧义命令在底栏报错。`help` 打开命令表弹窗。新增 `Runtime::prev_track`、
  `Runtime::seek` 与 `Runtime::set_loop_mode`。
- 实现 `Player::try_seek`（签名改为 `&self`）：清空输出环、执行解码器准确 seek、作废 EOF 状态并重设进度基准。
  `DecoderRuntime::try_seek` 清空前效果器输入环并发送 `AudioData::Seek`（不再做全量效果器 reset）；重采样器与交叉桥
  在 `Seek` 时丢弃缓冲状态。seek 后的显示进度与真实位置可能相差一个音频块。
- `←`/`→` 在当前曲目内跳转：点按 5 秒；长按期间重复事件只移动进度预览（黄色进度条 + `跳转预览`），
  最后一次重复后 150ms 提交目标。`n` 仍是下一曲。`Runtime::seek` 在换曲期间、或 EOF 尾部/交叉渐变尚未播完时忽略请求，
  因为该窗口内 `pos` 仍包含上一曲。
- `CrossBridge` 延迟线改为非对称填充：未达到配置的 1 秒时，每个输入块最多只输出一半（至少保留 1 个样本），
  因此播放可以低延迟启动，延迟随填充逐步增加；补满后恢复 1:1。seek 清空延迟线后立即恢复输出，延迟重新爬升，
  不再需要静音等待填充。交叉增益仍按完整时长预计算；尾长短于配置时长（延迟线未满时曲目结束）时缩放增益表索引，
  让 A 衰减到 0、B 升到 1，而不是截断曲线造成音量阶跃。
- 修复 Windows VT 输入：crossterm 0.29 会丢弃合成 VT 输入中的控制字符（ESC、CR），导致 `Enter` 无反应、方向键变成
  `[C` 文本、鼠标报文把 `[<...M` 塞进命令行。VT 模式下现在由专用线程直接读取 stdin 字节并喂给 `vtparse`；
  单独的 ESC 等待 50ms 超时后才作为 Esc 键，CR 与 LF 都映射为 Enter。
- 修复首次播放前 seek 导致后续播放失败：`Player::try_seek` 会往容量为 1 的 transport 通道塞一条 `PlayCmd::Seek`；
  流处于暂停时没人消费，下一次 `Play` 就返回 `Full`，播放无法开始。seek 不再发送该命令；解码器 seek 与效果链复位
  本来就在命令线程完成。
- 鼠标：底栏快捷键文字现在也是可点击按钮（退出、下一曲、快退/快进、播放/暂停、单行滚动、音量、Debug、命令模式），
  与原有的“点击列表行播放”并存。`PlayList::jump_to` 在目标已是当前曲目时也会把它从周期洗牌的待播队列中取出；
  目标不在队列中则不改动队列。

### 环形缓冲区

- `split_ring_buffer` 为 `T: Copy` 的写端（`LockedWriter` / `UnlockedWriter`）新增 `resize`。超过物理分配时重新分配
  A/B、阻止写端，按相同逻辑位置复制存量（复制期间读端继续输出，只有极短的交换窗口内读到“暂无数据”），交换缓冲区，旧缓冲区
  延迟到没有在途操作后释放；缩小或在物理分配内调整只更新逻辑容量。`capacity()` 返回当前容量，`Kind::Resizing` 表示扩容
  期间被拒绝的写入。

### 文件搜索队列

- `file_finder` 的目录工作队列改为 `ConcurrentRing<DirEntry>`，不再使用 `Mutex<VecDeque>` + `Condvar` + 运行线程计数。
  工作线程在 `pop_wait()` 上阻塞，收到 `WritersClosed` / `AllWaiting` 时退出，因此手动的计数增减与唤醒逻辑全部删除。
  新增 `FileFinder::new()`（以及 `Default`）替代单元结构体字面量，公共辅助函数加上 `#[must_use]`。

### 包 I/O

- `PackIO` 现在持有 `Box<dyn FileIo>` 而非具体 `File`。新增 `FileIo` 特征（`pack_io/file.rs`，为 `File` 实现：
  读/写/seek + `set_len`/`sync_data`/`lock`/`unlock`），使包层可以运行在非 `File` 后端上。会移动文件游标的操作
  （`manifest_data_block_read`、`set_pos_read`，以及管理器的 `load_pack_struct` / `load_pack_file_metadata`）改为接收
  `&mut self`。
- `PackVirtualFile` 从 `pack_io/file.rs` 移到新的 `pack_io/pack_file.rs`（`pack_io::file` 现在只放 `FileIo`）。
  `WBFilesPackManifest::file_mut()` 返回 `Option<&mut PackIO>`，`file_gc_add` 不再返回 `Result`。

### 构建

- 仓库通过 `rust-toolchain.toml` 固定 **nightly**（`channel = "nightly"`），因此此仓库中的所有 `cargo` 命令都使用
  nightly。`build_native.ps1` 在 `.cargo/native-config.toml` 基础上使用 `-Zbuild-std="std,panic_abort"` 执行本机优化
  release 构建。

### 模块布局

- 音乐播放引擎移入库 crate：`player`、`decoder_runtime`、`effect`（含 `runtime`/`resampler`/`cross_bridge`/`fade`）、
  `play_list`、`event_dispatch` 与 `tools` 现位于 `water_ball_tool_lib/src/music_play/`，通过
  `water_ball_tool_lib::music_play` 导出。
- 二进制 crate 只保留终端前端：`src/command/music_play.rs`（clap 参数 + TUI 接线）、
  `src/command/music_play/runtime.rs`（播放运行时）、`src/command/music_play/scanner.rs`（后台标签扫描）与
  `src/command/music_play/tui/`。
- `LoopMode` 移入库包并实现 `FromStr`。二进制包无法再为它派生 `clap::ValueEnum`（orphan rule），因此 `-m/--mode`
  改用 `value_parser` 解析；可选值保持不变。
- `debug-no-audio` feature 现定义在 `water_ball_tool_lib`（cpal 回调所在处），由二进制包转发，因此从仓库根执行
  `cargo build --features debug-no-audio` 仍然有效。
- `cpal`、`symphonia`、`rubato`、`crossbeam-channel` 从二进制包移至库包；二进制包只保留 `lofty` 用于读取标签。
- 运行库测试套件现在需要 `cargo test --workspace`（单独的 `cargo test` 只覆盖二进制包）。

### 日志与调试

- 终端日志改为异步：`TerminalWriter` 把整行放进有界通道（容量 1024），由 `log-writer` 线程取出并通过
  `MultiProgress::suspend()` 打印。调用线程只做 `try_send`，因此调试器停在断点（或控制台卡住）时，通道满只丢弃终端日志，
  不会让工作线程持着进度条写锁阻塞在日志里。文件日志仍为同步写。
- 第三方 `log` 记录压到 `warn`（`log_filter`）：`tracing-subscriber` 默认开启的 `tracing-log` 桥接会把 lofty 逐块的
  `log::debug!` 送进该 writer，而每条日志都会清屏并整块重绘所有进度条。
- `mp`：`add_track` 不再持 `play_list` 互斥锁读取 lofty 标签或调用 `next()`——首曲的 `next()` 会在同一线程上二次加锁
  （自死锁）。

### 曲目收集

- `mp` 遍历目录时只接收 `lofty` 能识别的扩展名（`FileType::from_path`），因此音乐库目录不会为每个无关文件都尝试一次标签读取。
  显式传入的文件不受该过滤影响，始终会尝试读取。

### 播放列表修复

- `PeriodicShuffle` 的周期边界：重排下一周期的队列时，正在播放的曲目可能被抽到首位，造成跨周期连续重复。现在重排后会把
  当前播放索引从队首挪开（新增 `avoid_immediate_repeat` 辅助函数）。只有一首曲目的列表无法避免重复，保持原样。
- `Shuffle` 使用 `random_range(0..len)`，但曲目索引是 1-based（`get_current_track` 取 `current_index - 1`）：索引 0 会
  下溢，且末曲永远播不到。范围改为 `1..=len`，单曲列表也能正常播放。

### 播放列表编辑与上一曲

- `PlayList` 新增有界播放轨迹（`VecDeque`，构造时按 `HISTORY_CAP = 1024` 预分配，长度从 0 开始）。只有 `next` 写入：
  曲目真正发生切换时压入离开的那一首（`RepeatOne` 的自动循环原地不动，不记录），写满后丢弃最旧的一条。
- 新增 `PlayList::prev()`：弹出轨迹并返回上一首，同时刷新 `next_id`（避免旧曲缓冲的 EOF 事件在新曲上触发自动换曲）。
  退到轨迹起点时返回 `None`；没有 redo 路径，之后的 `next` 按当前模式重新计算。
- 周期洗牌下，`prev` 还会把落点从本周期待播队列中移除，保证它不会在同一周期内再播一次。
- 新增编辑 API：`add_track` / `add_tracks`（追加）与 `insert_track` / `insert_tracks`（1-based 位置，越界夹到
  `1..=长度+1`）。批量版本是唯一实现，单曲版本转调它。
- 中间插入会同时后移 `current_index`、轨迹项和周期队列项；追加到末尾不涉及任何一项。周期洗牌进行中时，新曲会补进
  当前周期的待播队列。

### 文档

- `AGENTS.md` 只保留规范；各模块的架构说明移至 `ARCHITECTURE.md`，后者新增「环形缓冲区模块」章节（§12），涵盖
  `concurrent_ring` 与 `split_ring_buffer`。

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

### 解码错误恢复

- 解码循环对瞬时 IO 错误先重试（`DecoderRuntime::is_retryable_io_error`：`Interrupted`、`WouldBlock`、`TimedOut`、
  连接重置/中止，以及 Windows 1450 `ERROR_NO_SYSTEM_RESOURCES` 这类平台资源耗尽码），最多 5 次、每次间隔 200 ms，
  耗尽后才视为失败。
- 不可重试或重试耗尽的错误不再杀死解码线程：循环发出 `DecoderEvent` 错误事件，成对取出 `DecoderSession`，清空待写输出
  缓冲，然后回到 condvar 等待。线程保持存活，后续换曲仍能播放，不会再让解码器永久静音（实际案例：`next_packet` 以
  `os error 1450` 失败后播放彻底失效）。
- `DecoderRuntime` 暴露原子 `poisoned` 标志（`is_poisoned`）：上述不可恢复错误置位，换曲成功（`set_decoder_path`）自动清除，
  因此前端即使忘记解毒也不会把后续播放命令变成提前换曲。
- `mp` 通过 `DecoderEventChannelSink` 消费解码错误事件（在通道线程上；暂停播放并唤醒 UI 线程）。毒化期间进度条前缀显示
  `异常暂停中`，并在歌曲信息与时间统计块之间显示红色 `Error` 行；`Play`/`P` 在毒化时先换曲，随后必定恢复播放。

### 环形缓冲区半关闭（第 1 步）

- `ConcurrentRing` 的阻塞调用不再永久阻塞：每个阻塞方法（`push_wait`、`write_wait`、`write_all` 与 `pop_wait`、
  `read_wait`、`read_exact`）登记调用句柄的方向，句柄的 `Clone`/`Drop` 维护每方向计数。某一方向的最后一个句柄消失后，
  对向的阻塞调用返回新错误变体 `ReadersClosed` / `WritersClosed`；该方向重新出现登记的句柄即自动恢复。判定完全由方法自身
  与 `Clone`/`Drop` 完成——没有读/写端句柄类型、没有手动 close/reopen、也没有额外的关闭布尔值（计数即状态）。从未调用过
  阻塞方法的句柄不会被判为已关闭。

### 前效果器错误处理（第 2 步）

- 前效果器运行时线程遇到运行期错误不再退出：环错误先用 `Skip` 不连续信号复位效果链、发出
  `PreEffectRuntimeEventErrorKind::Runtime` 事件，再退避 10 ms 继续消费（写端关闭时因此是等待而不是空转）；输出写入
  硬错误同样只上报并丢弃该块。只有 `stop` 标志结束循环。
- `Runtime::create` 注册 `PreEffectRuntimeEventChannelSink`，仅记录这些错误（播放可继续，不暂停）。

### 崩溃看门狗（第 3 步）

- 新增库模块 `music_play/watchdog.rs`：被监视线程持有 `ThreadExitGuard`，其 `Drop`（含 panic unwind）在 `WatchNotify`
  中置位槽位并通过 `Condvar` 唤醒唯一的看门狗线程——不轮询 `JoinHandle::is_finished()`，也不需要第二个线程。
- `Player::new` 把通知器注入解码器与前效果器运行时，并启动 `player-watchdog`。线程退出时暂停播放（不自动恢复）、置解码器
  毒化标志让 `Play` 走换曲恢复，并重启线程；`DecoderRuntime::run` / `PreEffectRuntime::run` 现在会先回收已结束的
  `JoinHandle` 再重建。
- 前效果器重启会整条重建效果链：`PreEffectRuntime::set_chain_factory`（由 `Player::new` 注册）+
  `rebuild_effects()` / `set_effects()`，因为旧效果器可能已毒化；重启前还会清空输入环残留。
- `Player::try_stop` 先关停看门狗、最后 join 它。

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
- `split_ring_buffer`：阻塞交接可能极低概率死锁。发布方在发布新位置之后、于 `wait_mutex` 之外检查“是否有等待者”，
  等待方登记后再读取该位置；x86 的 StoreLoad 重排会让双方同时看不到对方，导致对端永久睡死（跨界容量阻塞测试约
  1/200 轮复现）。现在两侧握手都执行 `fence(SeqCst)`，“跳过唤醒”与“读到旧位置”不可能同时发生。热路径仍不取锁，
  实时音频回调保持无锁。
- `concurrent_ring`：`ConcurrentRingGuard`（`ring.lock()`）从不唤醒阻塞的对端，若唯一的进展来自 Guard 内的操作，
  阻塞在 `push_wait` / `pop_wait` 的线程会永远睡下去。Guard 存活期间无法唤醒（唤醒需 `wait_lock`，而锁序是
  `wait_lock → write_lock → read_gate.write`），因此改为在 `Drop` 中先释放两把锁、再唤醒读写两侧。

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
