//! wbfp 公共核心逻辑 / shared wbfp core helpers
//!
//! 经典进度条路径与 TUI 路径共用的路径推导、线程数解析、文件复制与解包函数。
//! 进度条相关的编排仍留在 `classic` / `ops` 两条路径中。
//!
//! Path deduction, thread-count resolution, file copy and extraction shared by the
//! classic progress-bar path and the TUI path. Progress orchestration stays in
//! `classic` / `ops`.

//AI=== 公共核心：经典路径与 TUI 路径共用 / shared core used by both paths
use std::fs::File;
use std::io::{Error, ErrorKind, Read, Write};
use std::num::NonZero;
use std::path::{Path, PathBuf};
use std::thread;
use tracing::{error, info, warn};
use water_ball_tool_lib::file_finder::FileInfo;
use water_ball_tool_lib::wb_files_pack::manager_sync::ManagerSync;

/// 线程数兜底值 / Fallback thread count.
///
/// 8 恒为合法非零值；`match` 在编译期求值，绝不 panic。
/// 8 is always a valid non-zero value; the `match` is evaluated at compile time and never panics.
pub(crate) const DEFAULT_THREAD_COUNT: usize = match NonZero::new(8) {
    Some(v) => v.get(),
    None => 8,
};

/// 复制结果 / Copy outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CopyOutcome {
    /// 已读取并写入 / read and written
    Written,
    /// 写入优化命中，未改动 / write optimization hit, left unchanged
    Unchanged,
    /// 打开或读写失败，已跳过 / open or I/O failure, skipped
    Failed,
}

impl CopyOutcome {
    /// 是否可视为成功 / whether the outcome counts as success
    pub(crate) fn is_success(self) -> bool {
        matches!(self, Self::Written | Self::Unchanged)
    }
}

/// 解析线程数；未指定时使用可用并行度，失败则回退常量。
///
/// Resolve the thread count; use available parallelism when omitted, falling back to the constant.
pub(crate) fn get_thread_count(thread_count: Option<usize>) -> usize {
    thread_count.unwrap_or_else(|| {
        let r = thread::available_parallelism().map_or(DEFAULT_THREAD_COUNT, NonZero::get);
        info!("未指定线程数量，将最多使用{r}个线程");
        r
    })
}

/// 推导打包输出路径 / Derive the pack output path.
pub(crate) fn pack_output_path(
    in_path: &Path,
    out_pack_path: Option<&str>,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let mut pack_path = if let Some(pack_path) = out_pack_path {
        let pack_path: &Path = pack_path.as_ref();
        if pack_path.exists() && pack_path.is_dir() {
            if let Some(name) = in_path.file_name() {
                pack_path.join(name)
            } else {
                let msg =
                    "提供的输入路径是目录，且无法获取输入路径文件名，无法自动设置输出文件路径";
                error!(msg);
                return Err(Error::other(msg).into());
            }
        } else {
            pack_path.to_path_buf()
        }
    } else {
        let pack_path = PathBuf::from(".");
        if let Some(name) = in_path.file_name() {
            pack_path.join(name)
        } else {
            let msg = "未提供输出路径，且无法获取输入路径文件名，无法自动设置输出文件路径。";
            error!(msg);
            return Err(Error::other(msg).into());
        }
    };
    pack_path.set_extension("wbfp");
    Ok(pack_path)
}

/// 推导解包输出目录 / Derive the unpack output directory.
pub(crate) fn unpack_out_dir(
    pack_path: &str,
    out_dir: Option<&str>,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if let Some(d) = out_dir {
        return Ok(PathBuf::from(d));
    }
    let stem = Path::new(pack_path).file_stem().ok_or_else(|| {
        Box::<dyn std::error::Error>::from(format!(
            "无法从包文件路径提取前缀以推导输出目录: {pack_path}"
        ))
    })?;
    let out = Path::new(".").join(stem);
    info!("未指定输出路径，将解包到: {}", out.display());
    Ok(out)
}

/// 将单个文件从磁盘复制到包文件的虚拟文件系统中。
///
/// 打开源文件 → 在包中创建虚拟文件 → 循环读写（缓冲区 `run_buf`）
/// 直至复制完成。权限不足或打开失败时记录错误并跳过该文件。
/// 通过 `update_pb` 回调通知外部更新进度，通过 `main_pb_tx`
/// 向主线程发送字节增量用于聚合总进度条。
///
/// Copies a single file from disk into the pack's virtual filesystem.
///
/// Opens source file → creates virtual file in pack → loops read/write
/// using `run_buf` buffer until complete. Logs and skips on permission
/// denied or open failure. Invokes `update_pb` callback for external
/// progress updates, and sends byte deltas to the main thread via
/// `main_pb_tx` for total progress bar aggregation.
pub(crate) fn copy_file_into_pack(
    pack_man: &mut ManagerSync,
    run_buf: &mut [u8],
    info: &FileInfo,
    this_in_path: &Path,
    this_pack_path: &Path,
    main_pb_tx: Option<&std::sync::mpsc::Sender<(u64, u64)>>,
    update_pb: &mut dyn FnMut(u64, u64, &FileInfo, &Path, &Path),
    write_optimization: bool,
) -> CopyOutcome {
    //尝试打开文件
    let mut in_file = match File::open(this_in_path) {
        Ok(file) => file,
        Err(err) => {
            let kind_str = match err.kind() {
                ErrorKind::PermissionDenied => "权限不足",
                ErrorKind::NotFound => "文件不存在",
                _ => {
                    error!(
                        r#"[未处理错误]无法打开文件"{}"，err:{err}"#,
                        this_in_path.display()
                    );
                    return CopyOutcome::Failed;
                }
            };
            error!(
                r#"无法打开文件"{}"，{kind_str}，将跳过，err:{err}"#,
                this_in_path.display()
            );
            return CopyOutcome::Failed;
        }
    };
    //尝试创建虚拟文件（已知大小：按 128B 精确分配，避免 4MiB 整块浪费）
    //Try creating the virtual file (known size: exact 128B-aligned allocation)
    let mut out_file = match pack_man
        .virtual_file_options()
        .write(true)
        .create_new(true)
        .alloc_size(Some(info.length()))
        .open(this_pack_path)
    {
        Ok(mut v) => {
            if let Err(err) = v.set_len(info.length()) {
                warn!(
                    r#"无法设置虚拟文件"{}"的大小，将继续，err: {err}"#,
                    this_pack_path.display()
                );
            }
            if let Err(err) = v.set_modified(info.modified_time()) {
                warn!(
                    r#"无法设置虚拟文件"{}"的修改时间，将继续，err: {err}"#,
                    this_pack_path.display()
                );
            }
            v
        }
        Err(err) => {
            //尝试打开文件
            match pack_man
                .virtual_file_options()
                .write(true)
                .open(this_pack_path)
            {
                Ok(mut v) => {
                    //基于检查修改时间和大小，简单的写入优化判断
                    if write_optimization
                        && (v.get_modified() == info.modified_time()
                            && v.get_len() == info.length())
                    {
                        update_pb(
                            info.length(),
                            info.length(),
                            info,
                            this_in_path,
                            this_pack_path,
                        );
                        //更新总进度条
                        if let Some(main_pb_tx) = main_pb_tx {
                            let _ = main_pb_tx.send((0, info.length()));
                        }
                        return CopyOutcome::Unchanged;
                    }
                    if let Err(err) = v.set_len(info.length()) {
                        warn!(
                            r#"无法设置虚拟文件"{}"的大小，将继续，err: {err}"#,
                            this_pack_path.display()
                        );
                    }
                    if let Err(err) = v.set_modified(info.modified_time()) {
                        warn!(
                            r#"无法设置虚拟文件"{}"的修改时间，将继续, err: {err}"#,
                            this_pack_path.display()
                        );
                    }
                    v
                }
                Err(err2) => {
                    error!(
                        r#"无法创建或打开虚拟文件"{}",将跳过, err: {err}\nerr2: {err2}"#,
                        this_pack_path.display()
                    );
                    return CopyOutcome::Failed;
                }
            }
        }
    };
    //写入操作
    //读多少写多少：短写时续写余量不丢弃；源文件搜索后增长也全部写入，
    //虚拟文件实际大小由写入自动同步；EOF（read 返回 0）为正常结束条件。
    let mut write_len = 0;
    let mut io_failed = false;
    loop {
        //读
        match in_file.read(run_buf) {
            Ok(0) => {
                //EOF 防御：未达到记录长度时警告而不是进入无限循环
                if write_len < info.length() {
                    warn!(
                        r#"文件"{}"提前结束（EOF），已写入{write_len}B，搜索时记录长度{}B"#,
                        this_in_path.display(),
                        info.length()
                    );
                }
                break;
            }
            Ok(this_read_len) => {
                let mut buf_offset = 0;
                let mut write_err = false;
                while buf_offset < this_read_len && !write_err {
                    match out_file.write(&run_buf[buf_offset..this_read_len]) {
                        Ok(0) => {
                            //防御：写入无进展，避免无限循环
                            warn!(
                                r#"写入虚拟文件"{}"返回0字节，将跳过剩余数据，err: 写入无进展"#,
                                this_pack_path.display()
                            );
                            write_err = true;
                        }
                        Ok(this_write_len) => {
                            buf_offset += this_write_len;
                            write_len += this_write_len as u64;
                            //更新进度
                            update_pb(
                                write_len,
                                this_write_len as u64,
                                info,
                                this_in_path,
                                this_pack_path,
                            );
                            if let Some(main_pb_tx) = main_pb_tx {
                                let _ = main_pb_tx.send((0, this_write_len as u64));
                            }
                        }
                        Err(err) => {
                            error!(
                                r#"写入虚拟文件"{}"错误, 将跳过，err:{err}"#,
                                this_pack_path.display()
                            );
                            write_err = true;
                        }
                    }
                }
                //写入出错或无进展时结束本文件
                if write_err {
                    io_failed = true;
                    break;
                }
            }
            Err(err) => {
                error!(
                    r#"读取文件"{}"失败，将跳过，err:{err}"#,
                    this_in_path.display()
                );
                io_failed = true;
                break;
            }
        }
    }

    if io_failed {
        CopyOutcome::Failed
    } else {
        CopyOutcome::Written
    }
}

/// 将包中的单个虚拟文件提取到磁盘。
///
/// 大文件（>512MiB）单独记录 info 日志提示。打开虚拟文件 → 创建磁盘文件
/// → 预分配空间 → 循环读写。通过 `progress` 闭包报告每块读写后的进度。
///
/// Extracts a single virtual file from the pack to disk.
///
/// Large files (>512MiB) are logged with an info message. Opens the
/// virtual file → creates the disk file → pre-allocates space → loops
/// read/write. Reports progress after each chunk via the `progress` closure.
pub(crate) fn extract_pack_file_to_disk(
    pack: &mut ManagerSync,
    pack_path: &Path,
    out_path: &Path,
    run_buf: &mut [u8],
    progress: Option<&dyn Fn(u64, u64)>,
) -> std::io::Result<u64> {
    let mut in_file = match pack.open_virtual_file(&pack_path) {
        Ok(v) => v,
        Err(err) => {
            error!("无法打开虚拟文件{:?}，将跳过，err:{err}", pack_path);
            return Err(Error::other(err));
        }
    };

    let file_len = in_file.get_len();

    if file_len > 512 * 1024 * 1024 {
        info!(
            r#"正在从包文件虚拟路径"{}"复制大文件到"{}"，大小:{}[{}]"#,
            pack_path.display(),
            out_path.display(),
            water_ball_tool_lib::tools::bytes_len_to_string(file_len),
            file_len
        );
    }

    let mut out_file = match File::create(out_path) {
        Ok(f) => f,
        Err(err) => {
            error!("无法创建文件{:?},将跳过，err：{err}", out_path);
            return Err(err);
        }
    };

    if let Err(err) = out_file.set_len(file_len) {
        warn!(
            "无法对输出文件{:?}进行预分配空间，将继续, err:{err}",
            out_path
        );
    }

    let mut write_len = 0u64;
    while write_len < file_len {
        match in_file.read(run_buf) {
            Ok(this_read_len) => {
                if this_read_len == 0 {
                    warn!("虚拟文件{:?}读取的大小为0, 将跳过。", pack_path);
                    break;
                }
                match out_file.write(&run_buf[..this_read_len]) {
                    Ok(this_write_len) => {
                        if this_write_len != this_read_len {
                            warn!(
                                "虚拟文件{:?}写入文件{:?}大小不一致，读：{this_read_len}，写：{this_write_len}",
                                pack_path, out_path
                            );
                        }
                        write_len += this_write_len as u64;
                        if let Some(progress) = progress {
                            progress(write_len, file_len);
                        }
                    }
                    Err(err) => {
                        error!("写入文件{:?}错误, 将跳过，err:{err}", pack_path);
                        break;
                    }
                }
            }
            Err(err) => {
                error!("读取虚拟文件{:?}失败，将跳过，err:{err}", pack_path);
                break;
            }
        }
    }

    Ok(write_len)
}

//AI_END===
