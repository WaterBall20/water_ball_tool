# 水球工具 WaterBall Tool

> **语言**: [English](../README.md) | 简体中文

> **个人专门用于长期学习练习的项目**
>
> 部分代码由 AI (deepseek-v4-pro) 生成

---

## 简介

一个多合一命令行工具箱，使用 **Rust** 编写。目前包含三个核心功能：

- **文件搜索器** (`ff`) — 多线程并行扫描目录树，输出 JSON 格式文件清单。支持符号链接循环检测与可选的逐文件哈希
  （MD5/SHA-1/SHA-2/BLAKE2b/BLAKE3/CRC32）。
- **水球包文件** (`wbfp`) — 自定义二进制归档格式，支持打包/解包/BLAKE3 哈希校验。
- **音乐播放器** (`mp`) — 终端音乐播放器，可播放音频文件与目录：支持播放列表模式、音量控制与 stdin 交互命令循环。
  `tui` 界面仍在开发中（尚未接入命令）。

---

## 安装

```bash
# 克隆仓库
git clone <repo-url>
cd water_ball_tool

# 编译（debug）
cargo build

# 编译（release，推荐）
cargo build --release

# 运行
./target/release/water_ball_tool -h
```

仓库是 cargo workspace：根包为 `water_ball_tool` 二进制，`water_ball_tool_lib` 为库 crate。在仓库根执行构建会同时构建两者。

---

## 用法

### 文件搜索 `ff`

```bash
# 搜索当前目录
water_ball_tool ff .

# 搜索指定目录并输出 JSON
water_ball_tool ff ./src result.json

# 跳过符号链接
water_ball_tool ff /path/to/dir output.json -s

# 计算哈希并写入 JSON 输出（位置参数，位于输出路径之后）
water_ball_tool ff ./src result.json sha256 blake3

# 一次性计算全部支持的哈希
water_ball_tool ff ./src result.json all
```

`HASH` 是可选的多值位置参数（依次为路径、输出路径、哈希）。可选值：`all`、`md5`、`sha256`、`sha512`、`sha1`、
`blake3`、`blake2b`、`crc32`（别名：`*`、`m5`、`s256`、`s512`、`s1`、`b3`、`b2b`、`crc`）。开启哈希时文件发现被强制为单
线程；哈希计算本身由独立的工作线程池执行。

### 打包 `wbfp p`

```bash
# 将目录打包为水球包文件（默认分离清单为 .wbm）
water_ball_tool wbfp p ./src my.pack

# 不分离清单（所有数据在单个 .pack 文件中）
water_ball_tool wbfp p ./src my.pack -n
```

### 解包 `wbfp u`

```bash
water_ball_tool wbfp u my.pack ./output_dir
# 解包前逐个文件进行 BLAKE3 哈希校验，校验失败的文件跳过并报警告：
water_ball_tool wbfp u -H my.pack ./output_dir
```

### 哈希校验 `wbfp h`

```bash
water_ball_tool wbfp h my.pack
```

### 音乐播放器 `mp`

```bash
# 播放文件或目录（默认模式：sequential-loop）
water_ball_tool mp ./music

# 初始音量（0.0–1.0，默认 0.75）与随机播放
water_ball_tool mp ./music -v 0.5 -m shuffle

# 播放模式：sequential-loop（默认）、sequential、repeat-one、shuffle、periodic-shuffle
water_ball_tool mp ./music --mode repeat-one

# 详细输出（仅长选项）
water_ball_tool mp ./music --verbose
```

开始播放后，命令从 stdin 读取指令：`next`/`n`（下一曲）、`Play`/`P`（播放）、`pause`/`p`（暂停）、`exit`（停止并退出）。
音量超出 0.0–1.0 时会在开始播放前返回参数错误。

---

## 水球包文件格式

自行设计的二进制归档格式，主要特点：

| 特性             | 说明                                                                                                                                                                                                       |
|------------------|------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| **A/B 原子写入** | 清单数据块采用双块交替写入，崩溃后至少保留一个完整版本                                                                                                                                                     |
| **BLAKE3 哈希**  | 文件数据完整性校验                                                                                                                                                                                         |
| **增量保存**     | 跟踪 dirty 标记，仅写入变更部分                                                                                                                                                                            |
| **垃圾回收**     | 自动合并相邻空闲块，复用释放空间                                                                                                                                                                           |
| **虚拟文件增删** | `create_dir_all`/`create_file` 创建，`delete_file`/`delete_dir_all`/`erase_file`/`erase_dir_all` 删除/擦除：删除仅移除元数据（数据块回收不覆写），擦除先覆写至磁盘再移除（支持 Zero/Random/DoD 5220 策略） |
| **渐进式保存**   | 每写入一些数据或一些文件后再自动保存一次                                                                                                                                                                   |
| **进程写锁**     | `.lock` 文件 + PID 实现进程级互斥                                                                                                                                                                          |
| **清单分离**     | 可选将索引数据分离为 `.wbm` 文件                                                                                                                                                                           |

格式规范：[`wb_files_pack/manifest-data.md`](wb_files_pack/manifest-data.md)

---

## 架构

```
water_ball_tool_lib/src/          # 库 crate
├── lib.rs                        # 模块导出（file_finder / wb_files_pack / tools / concurrent_ring / split_ring_buffer / gakumasu）
├── file_finder.rs                # 多线程文件搜索器 + inode 链循环检测
├── file_finder/
│   └── test.rs                   # 文件搜索器单元测试（符号链接场景）
├── wb_files_pack.rs              # 水球包文件模块入口
├── wb_files_pack/
│   ├── data.rs                   # 核心数据结构（Attribute/PackStruct/ManifestDataBlock 等）
│   ├── error.rs                  # 错误类型
│   ├── manager.rs                # 包文件管理器（创建/打开/保存/GC/锁/删除/擦除）
│   ├── manager/
│   │   ├── delete.rs             # 虚拟文件/目录删除与擦除实现
│   │   └── test.rs               # 内部 API 测试
│   ├── manager_sync.rs           # 线程安全同步管理器（Arc<Mutex<>>）
│   ├── manager_sync/test.rs      # 外部 API 测试
│   ├── pack_io.rs                # 底层文件 I/O + 空间分配 + GC
│   ├── pack_io/
│   │   ├── file.rs               # 虚拟文件读写器（PackVirtualFile，含访问模式强制）
│   │   ├── file_handle.rs        # 核心虚拟文件逻辑
│   │   └── file_hash.rs          # BLAKE3 状态机
│   └── net_server.rs             # 网络服务（开发中）
├── tools.rs                      # 工具函数（路径转换、字节格式化、测试辅助）
├── concurrent_ring.rs            # 可并发初始化的环形缓冲区
├── split_ring_buffer.rs          # 读写分离有锁/无锁环形缓冲区（含 reader/writer/error/mode 子模块）
└── gakumasu/                     # 学园偶像大师游戏模拟器（开发中）
    ├── data.rs
    └── simulator.rs

src/                              # 二进制 crate（water_ball_tool）
├── main.rs                       # 入口点、CLI 分发、日志初始化
├── command.rs                    # CLI 命令路由 + 进度条辅助函数
└── command/
    ├── ff.rs                     # ff 子命令实现（搜索 + 可选哈希）
    ├── wbfp.rs                   # wbfp 子命令实现（打包/解包/哈希校验）
    ├── test.rs                   # 命令级集成测试（含符号链接循环检测）
    └── music_play.rs             # mp 子命令（终端音乐播放器）
        └── music_play/
            ├── player.rs         # cpal 输出流 + PlayCmd transport + Player::try_stop
            ├── decoder_runtime.rs # symphonia 解码线程
            ├── effect.rs         # 效果器 trait / 音频格式类型
            ├── effect/           # resampler.rs、cross_bridge.rs、fade.rs、runtime.rs（前效果器线程）
            ├── play_list.rs      # 曲目列表 + 循环/洗牌模式
            ├── runtime.rs        # 播放编排 + UI 刷新
            ├── event_dispatch.rs # 面向实时回调的专用事件/日志分发线程
            ├── tools.rs          # 时间统计 / 耗时累加器
            └── tui/              # 终端 UI（开发中：app.rs、ui.rs、channel_writer.rs）
```

**实现说明**

- `mp` 把解码器的 symphonia 格式读取器/解码器与其轨道元数据合并为一个 `DecoderSession`，由同一把锁保护；换曲时整体替换或
  取出会话，因此不会出现「有 engine 但缺少元数据」的状态。
- 命令层线程（`ff`/`wbfp`/`mp`）统一用 `thread::Builder` 创建；spawn 失败必须作为错误返回（绝不 panic，也绝不只记日志后
  继续）。`mp --volume` 超出 0.0–1.0 时返回参数错误。
- 只有在内部不变量被破坏时才可达的分支用 `unreachable!` 中止（逻辑错误）；运行期真会发生的失败（I/O 错误、spawn 失败、
  非法 CLI 输入）必须返回或上报。

另见：[`ARCHITECTURE.md`](ARCHITECTURE.md)

---

## 构建与测试

```bash
# 运行全部测试（跳过长时间测试，#[ignore] 自动处理）
cargo test

# 运行包括长时间测试（主分支 CI 全量测试）
cargo test -- --include-ignored

# 仅编译检查
cargo check

# 代码检查
cargo clippy

# 本机优化 release 构建（target-cpu=native，产物在 target/native/）
cargo --config .cargo/native-config.toml build --release

# 跳过音频读取/效果/音量的调试构建（mp 调试辅助）
cargo build --features debug-no-audio
```

`.cargo/native-config.toml` 不会自动生效——需如上通过 `--config` 传入。

---

## 技术栈

- **语言**: Rust (edition 2024)
- **序列化**: serde / serde_json
- **哈希**: blake3 / sha1 / sha2 / blake2b_simd / md-5 / crc32fast
- **进度条**: indicatif
- **日志**: tracing / tracing-subscriber
- **进程检测**: sysinfo
- **音频**: cpal / symphonia / rubato / lofty
- **终端 UI**: ratatui
- **通道**: crossbeam-channel

---

部分代码由 AI (deepseek-v4-pro) 生成
