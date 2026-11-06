//! wbfp 经典进度条路径 / wbfp classic progress-bar path
//!
//! 保留原有打包、解包和哈希校验逻辑：多线程写入（生产者-消费者模式，通过
//! `FileFinder::search_stream` 流式发现文件）、每工作线程独立进度条，以及大文件（>512MiB）日志提示。
//! 终端 TUI 路径见 `ops` / `tui`。
//!
//! Preserves the original pack, unpack and hash-verify logic: multi-threaded writing
//! (producer-consumer via `FileFinder::search_stream`), per-worker progress bars and
//! large-file (>512MiB) log notices. The terminal TUI lives in `ops` / `tui`.

//AI=== 经典进度条路径（从 wbfp.rs 拆出，保留原有行为）/ classic progress-bar path (split out of wbfp.rs, behavior preserved)

//AI=== 引入非 panic 的进度条样式构造辅助，替换各处进度条模板的强制取值
// Import the non-panicking progress-style helper, replacing panicking template unwraps.
use crate::command::{
    BUF_LEN, PACK_PROGRESS_STYLE_TEMPLATE, SPINNER_TEMPLATE, create_pb, progress_style,
};
//AI_END===
use super::common;
use super::{
    WaterBallFilePackCommand, WaterBallFilePackCommands, WaterBallFilePackCommandsHashVerify,
    WaterBallFilePackCommandsPack, WaterBallFilePackCommandsUnpack,
};
use crate::multi_progress_init;
use clap::error::Result;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::io::Error;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::{fs, io};
use tracing::{error, info, warn};
use water_ball_tool_lib::concurrent_ring::{ConcurrentRing, ConcurrentRingErrorKind};
use water_ball_tool_lib::file_finder::{FileFinder, FileInfo, FileKind, SearchEvent};
use water_ball_tool_lib::tools::PathTool;
use water_ball_tool_lib::wb_files_pack::manager_sync::ManagerSync;
use water_ball_tool_lib::wb_files_pack::{PackFileError, PackStructItemType};

type ArcMutex<T> = Arc<Mutex<T>>;

/// wbfp 命令路由入口，根据子命令分发到对应的执行函数。
/// 每个执行函数独立处理进度条和多线程逻辑。
///
/// wbfp command router. Dispatches to the appropriate execution function
/// based on the subcommand. Each function handles its own progress bars
/// and multi-threading logic independently.
pub(super) fn commands(args: WaterBallFilePackCommand) -> Result<(), Box<dyn std::error::Error>> {
    let mp = multi_progress_init("wbfp");
    info!("wbfp: 经典路径 / classic path");
    let args2 = &args.commands;
    match args2 {
        WaterBallFilePackCommands::Pack(p) => WaterBallFilePackArgsRuning::wbfp_p(p, mp),
        WaterBallFilePackCommands::Unpack(u) => WaterBallFilePackArgsRuning::wbfp_u(u, mp),
        WaterBallFilePackCommands::HashVerify(h) => WaterBallFilePackArgsRuning::wbfp_h(h, mp),
        //AI=== 经典路径不支持浏览，入口层已把 browse 限定到 TUI / the classic path cannot browse; the entry routes browse to the TUI
        WaterBallFilePackCommands::Browse(_) => {
            Err("browse 需要交互式终端 / browse requires an interactive terminal".into())
        } //AI_END===
    }
}

/// wbfp 命令的执行器，包含所有打包/解包/校验的运行时逻辑。
///
/// Execution engine for wbfp commands, containing all runtime logic
/// for pack, unpack, and hash verify operations.
struct WaterBallFilePackArgsRuning;

impl WaterBallFilePackArgsRuning {
    /// 打包目录为水球包文件。
    ///
    /// 参数: `<输入目录> <包文件路径> [-n]`
    ///
    /// 使用 `FileFinder` 多线程扫描源目录，将每个文件写入包的虚拟文件系统。
    /// `-n` / `--no-separation` 标志强制将清单数据嵌入 `.pack` 文件而非分离的 `.wbm` 文件。
    ///
    /// Pack a directory into a WaterBall pack file.
    ///
    /// Args: `<input_dir> <pack_path> [-n]`
    ///
    /// Uses `FileFinder` to multi-threaded scan the source directory, then writes
    /// each file into the pack's virtual filesystem. The `-n` / `--no-separation`
    /// flag forces manifest data to be embedded in the `.pack` file instead of a
    /// separate `.wbm` file.
    pub fn wbfp_p(
        args: &WaterBallFilePackCommandsPack,
        mp: MultiProgress,
    ) -> Result<(), Box<dyn std::error::Error>> {
        //源目录路径
        let in_path = PathBuf::from(&args.in_path);
        //输出的包文件路径
        let pack_path = common::pack_output_path(&in_path, args.out_pack_path.as_deref())?;
        //分离数据文件
        let separate_manifest = !args.no_separation;
        //写入优化
        let write_optimization = !args.no_write_optimization;
        //搜索进度条
        let ff_pb = create_pb(&mp);

        //包文件进度条
        let wb_pb = Arc::new(Mutex::new(create_pb(&mp)));

        info!("开始准备打包");

        let data_len = Arc::new(AtomicU64::new(0));

        let pack = {
            if pack_path.exists() {
                info!("包文件已存在，将打开并修改");
                if write_optimization {
                    info!("已启用写入优化，未更改的文件将跳过");
                }
                //AI=== 打开包文件失败向上传播错误，不再 panic
                // Propagate the open error instead of panicking.
                ManagerSync::options()
                    .read(true)
                    .write(true)
                    .open(&pack_path)?
                //AI_END===
            } else {
                info!("创建新包文件并初始化");
                //AI=== 创建包文件失败向上传播错误，不再 panic
                // Propagate the create error instead of panicking.
                ManagerSync::options()
                    .write(true)
                    .create_new(true)
                    .separate_manifest(separate_manifest)
                    .open(&pack_path)?
                //AI_END===
            }
        };
        //复制操作===
        Self::wbfp_p_run(
            wb_pb,
            data_len,
            &in_path,
            ff_pb,
            pack,
            write_optimization,
            args,
            &mp,
            &pack_path,
        )
    }

    fn wbfp_p_run(
        wb_pb: ArcMutex<ProgressBar>,
        data_len: Arc<AtomicU64>,
        in_path: &Path,
        ff_pb: ProgressBar,
        mut pack: ManagerSync,
        write_optimization: bool,
        args: &WaterBallFilePackCommandsPack,
        mp: &MultiProgress,
        pack_path: &Path,
    ) -> Result<(), Box<dyn std::error::Error>> {
        //复制操作===
        info!("开始复制数据");
        //设置为包文件具体进度条
        {
            let pb = wb_pb
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            pb.set_length(data_len.load(Ordering::Acquire));
            pb.set_style(
                ProgressStyle::default_bar()
                    .template(PACK_PROGRESS_STYLE_TEMPLATE)?
                    .progress_chars("=>-"),
            );
            pb.set_prefix("总进度");
        }

        //判断输入路径是否是目录或文件
        if in_path.is_file() {
            //文件直接复制
            info!("输入路径是文件，跳过搜索，直接复制");
            //移除搜索进度条
            drop(ff_pb);
            let mut file_name = in_path
                .file_name()
                .unwrap_or_default()
                .to_str()
                .unwrap_or_default()
                .to_string();
            file_name.push_str(".wbfp");
            let file_metadata = in_path.metadata()?;
            let file_info = FileInfo::new(
                file_name.clone(),
                file_metadata.len(),
                FileFinder::get_file_modified(&file_metadata),
                FileKind::File { hash: None },
            );
            let mut run_buf = vec![0u8; 1024 * 1024];
            let mut write_len = 0;
            let mut last_write_len = 0;
            let mut update_pb =
                |write_len_2, _, info: &FileInfo, this_in_path: &Path, this_pack_path: &Path| {
                    write_len = write_len_2;
                    if write_len - last_write_len >= 5 * 1024 * 1024 {
                        let pb = wb_pb
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        pb.set_length(info.length());
                        pb.set_position(write_len);
                        pb.set_message(format!(
                            "\tFile path: {}\n\tPack path: {}",
                            this_in_path.display(),
                            this_pack_path.display()
                        ));
                        last_write_len = write_len;
                    }
                };

            let _ = common::copy_file_into_pack(
                &mut pack,
                &mut run_buf,
                &file_info,
                in_path,
                file_name.as_ref(),
                None,
                &mut update_pb,
                write_optimization,
            );
        } else {
            //复制时所用的线程数
            let thread_count = common::get_thread_count(args.thread_count);

            //搜索文件===
            info!("开始搜索文件");
            warn!("目前搜索将跳过符号链接");
            //搜索结果队列
            let results = ConcurrentRing::new();
            //搜索结束标志
            let ff_end = Arc::new(AtomicBool::new(false));
            //文件计数
            let file_count = Arc::new(AtomicU64::new(0));

            //创建专门的线程搜索
            //AI=== 搜索线程创建失败向上传播（原 thread::spawn 会 panic）/ propagate search-thread spawn failure (thread::spawn would panic)
            let ff_thread = Self::wbfp_p_search(
                in_path,
                results.clone(),
                &ff_end,
                &file_count,
                &data_len,
                ff_pb,
            )?;
            //AI_END===

            //复制线程
            //AI=== 复制线程创建失败向上传播（原 thread::spawn 会 panic）/ propagate copy-thread spawn failure (thread::spawn would panic)
            let wp_thread = Self::wbfp_p_copy(
                &pack,
                results,
                in_path,
                &ff_end,
                thread_count,
                mp,
                write_optimization,
                wb_pb,
                data_len,
                file_count,
            )?;
            //AI_END===
            //等待线程结束
            //AI=== 线程 join 失败（线程 panic）转为错误传播，不再 expect panic
            // Convert a failed join (thread panicked) into an error instead of panicking.
            if let Err(e) = ff_thread
                .join()
                .map_err(|_| Error::other("文件搜索线程异常退出 / file search thread panicked"))?
            {
                error!("文件搜索线程错误：{e}");
            }
            ff_end.store(true, Ordering::Release);
            wp_thread
                .join()
                .map_err(|_| Error::other("复制线程异常退出 / copy thread panicked"))??;
            //AI_END===
        }
        info!("操作已完成,文件保存到{}", pack_path.display());
        Ok(())
    }

    ///打包搜索线程
    //AI=== 线程创建失败必须返回错误：返回 io::Result<JoinHandle<...>>，创建失败由调用方用 ? 传播
    // Thread creation failure MUST be returned: return io::Result<JoinHandle<...>> so the caller
    // propagates it with `?`.
    fn wbfp_p_search(
        in_path: &Path,
        results: ConcurrentRing<(PathBuf, FileInfo)>,
        ff_end: &Arc<AtomicBool>,
        file_count: &Arc<AtomicU64>,
        data_len: &Arc<AtomicU64>,
        ff_pb: ProgressBar,
    ) -> io::Result<JoinHandle<io::Result<()>>> {
        //AI_END===
        use std::thread;

        //创建专门的线程搜索
        {
            let in_dir_path = in_path.to_path_buf();
            let results = results.clone();
            let ff_end = ff_end.clone();
            let file_count = file_count.clone();
            let data_len = data_len.clone();

            //AI=== 线程创建失败必须返回错误：Builder::new().name(...).spawn(...)，禁止会 panic 的 thread::spawn
            // Thread creation failure MUST be returned: Builder::new().name(...).spawn(...); the
            // panicking thread::spawn is forbidden.
            let worker = move || -> io::Result<()> {
                let ff = FileFinder;
                let dir_count = Arc::new(AtomicU64::new(0));
                //搜索进度
                let (tx, rx) = mpsc::channel();
                let (search_stream_handle, stream_results) =
                    ff.search_stream(&in_dir_path, true, tx, 2)?;
                //更新进度线程
                {
                    let file_count = file_count.clone();
                    let dir_count = dir_count.clone();
                    //AI=== 线程创建失败必须返回错误：用 ? 传播（所在闭包返回 io::Result）
                    // Thread creation failure MUST be returned: propagate with `?` (the enclosing closure returns io::Result).
                    let progress_worker = move || {
                        for (add_file, add_dir) in rx {
                            file_count.fetch_add(add_file, Ordering::Release);
                            dir_count.fetch_add(add_dir, Ordering::Release);

                            let all_count = file_count.load(Ordering::Acquire)
                                + dir_count.load(Ordering::Acquire);
                            ff_pb.set_position(all_count);
                            //10的倍数才更新
                            if all_count.is_multiple_of(100) {
                                ff_pb.set_message(format!(
                                    "[搜索文件]已发现 {} 文件和 {} 个目录",
                                    file_count.load(Ordering::Acquire),
                                    dir_count.load(Ordering::Acquire),
                                ));
                            }
                        }
                    };
                    thread::Builder::new()
                        .name("wbfp-pack-search-progress".to_string())
                        .spawn(progress_worker)?;
                    //AI_END===
                }

                for event in stream_results {
                    match event {
                        SearchEvent::Entry(path, info) => {
                            //只有文件才会加入队列
                            if let FileKind::File { .. } = info.file_kind() {
                                data_len.fetch_add(info.length(), Ordering::Release);
                                let _ = results.push((path, info));
                            }
                        }
                        SearchEvent::Warning(warning) => {
                            warn!(
                                "文件搜索警告 [{:?}]: {:?}",
                                warning.warning_type, warning.path
                            );
                        }
                    }
                }

                //AI=== 搜索子线程 join 失败转为 io 错误传播 / propagate join failure as io error
                search_stream_handle.join().map_err(|_| {
                    Error::other("文件搜索子线程异常退出 / search stream thread panicked")
                })??;
                //AI_END===
                info!(
                    "文件搜索已完成: {}个文件，{}个目录, 大小:{}",
                    file_count.load(Ordering::Acquire),
                    dir_count.load(Ordering::Acquire),
                    data_len.load(Ordering::Acquire)
                );
                ff_end.store(true, Ordering::Release);
                Ok(())
            };
            thread::Builder::new()
                .name("wbfp-pack-search".to_string())
                .spawn(worker)
            //AI_END===
        }
    }

    //AI=== 线程创建失败必须返回错误：返回 io::Result<JoinHandle<...>>，创建失败由调用方用 ? 传播
    // Thread creation failure MUST be returned: return io::Result<JoinHandle<...>> so the caller
    // propagates it with `?`.
    fn wbfp_p_copy(
        pack: &ManagerSync,
        results: ConcurrentRing<(PathBuf, FileInfo)>,
        in_path: &Path,
        ff_end: &Arc<AtomicBool>,
        thread_count: usize,
        mp: &MultiProgress,
        write_optimization: bool,
        wb_pb: ArcMutex<ProgressBar>,
        data_len: Arc<AtomicU64>,
        file_count: Arc<AtomicU64>,
    ) -> io::Result<JoinHandle<io::Result<()>>> {
        //AI_END===
        use std::thread;
        let pack = pack.clone();
        let results = results.clone();
        let in_dir_path = in_path.to_path_buf();
        let ff_end = ff_end.clone();

        //创建线程进度条
        let mut wb_pbs = Vec::with_capacity(thread_count);
        for _ in 0..thread_count {
            let pb = create_pb(mp);

            //AI=== 使用非 panic 的样式构造辅助，模板非法时降级为默认样式
            // Use the non-panicking style helper; fall back to the default style on bad templates.
            pb.set_style(progress_style(PACK_PROGRESS_STYLE_TEMPLATE).progress_chars("=>-"));
            //AI_END===

            wb_pbs.push(pb);
        }

        //AI=== 线程创建失败必须返回错误：Builder::new().name(...).spawn(...)，禁止会 panic 的 thread::spawn
        // Thread creation failure MUST be returned: Builder::new().name(...).spawn(...); the
        // panicking thread::spawn is forbidden.
        let worker = move || -> io::Result<()> {
            let (tx, rx) = mpsc::channel();
            let mut thread_handle = Vec::with_capacity(thread_count);
            //工作线程创建失败暂存，待已派发线程 join 完成后再向上返回 / stash the spawn failure, returned after joining dispatched workers
            let mut spawn_error: Option<Error> = None;
            //
            for index in 0..thread_count {
                let main_pb_tx = tx.clone();
                let pack_man = pack.clone();
                let files_list = results.clone();
                let in_dir_path = in_dir_path.clone();
                //AI=== 逻辑错误：派发循环次数与进度条数量一致，pop 失败说明不变量被破坏
                // 逻辑错误：每个工作线程在派发前都已创建对应进度条，`pop` 必然成功。
                // Logic error: one progress bar is created per dispatched worker, so `pop` must succeed.
                let Some(pb) = wb_pbs.pop() else {
                    unreachable!(
                        "逻辑错误：打包工作线程缺少进度条 / logic error: pack worker has no progress bar"
                    );
                };
                //AI_END===
                let ff_end = ff_end.clone();
                pb.set_prefix(format!("线程{index}"));
                //AI=== 工作线程创建失败必须返回错误：Builder + 暂存错误，禁止会 panic 的 thread::spawn
                // Worker-creation failure MUST be returned: Builder + stashed error; the panicking
                // thread::spawn is forbidden.
                match thread::Builder::new()
                    .name(format!("wbfp-pack-worker-{index}"))
                    .spawn(move || {
                        Self::wbfp_p_work(
                            pack_man,
                            pb,
                            files_list.clone(),
                            &in_dir_path,
                            &ff_end,
                            &main_pb_tx,
                            write_optimization,
                        );
                    }) {
                    Ok(handle) => thread_handle.push(handle),
                    Err(e) => {
                        error!(
                            "创建工作线程失败，停止派发剩余工作线程 / failed to spawn a worker, stopping dispatch: {e}"
                        );
                        spawn_error = Some(e);
                        break;
                    }
                }
                //AI_END===
            }

            drop(tx); //释放用不到的发送器，否则死锁。
            //计数变量
            let mut write_file_count = 0;
            let mut write_len = 0;
            let mut last_write_len = 0;
            //更新总进度
            for (file_count_add, len_add) in rx {
                write_file_count += file_count_add;
                write_len += len_add;
                //更新进度
                if write_len - last_write_len > 10 * 1024 * 1024 {
                    let pb = wb_pb
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    //总大小与实际写入量对齐（源文件可能在复制期间增长）
                    let mut total = data_len.load(Ordering::Acquire);
                    if write_len > total {
                        total = write_len;
                    }
                    pb.set_length(total);
                    pb.set_position(write_len);
                    pb.set_message(format!(
                        "[{write_file_count}/{}个文件]",
                        file_count.load(Ordering::Acquire),
                    ));
                    last_write_len = write_len;
                }
            }

            //等待所有工作线程结束
            //AI=== 工作线程 join 失败仅记录错误，主聚合线程不 panic
            // A failed worker join only logs an error; the aggregation thread does not panic.
            for item in thread_handle {
                if item.join().is_err() {
                    error!("工作线程异常退出（panic），已忽略其余结果");
                }
            }
            //AI_END===
            //AI=== 工作线程创建失败在所有已派发线程 join 完成后向上传播
            // Propagate the worker-creation failure after all dispatched workers have been joined.
            if let Some(e) = spawn_error {
                return Err(e);
            }
            Ok(())
            //AI_END===
        };
        // 汇总线程创建失败同样向上传播 / the aggregator thread also propagates creation failures
        thread::Builder::new()
            .name("wbfp-pack-copy".to_string())
            .spawn(worker)
        //AI_END===
    }

    /// 工作线程的核心循环：从共享文件队列中取文件并写入包文件。
    ///
    /// 队列为空时通过条件变量挂起等待，搜索线程发现新文件时唤醒。
    /// 搜索结束（`ff_end` 为 true 且队列为空）时退出循环。
    /// 每写 5MiB 更新一次所属线程的独立进度条，同时通过 `main_pb_tx`
    /// 向主线程发送已写字节数用于聚合总进度。
    ///
    /// Core worker loop: pop files from the shared queue and write them
    /// into the pack. Suspends via condition variable when the queue is
    /// empty and wakes when the search thread discovers new files. Exits
    /// when search finishes (ff_end is true) and the queue is drained.
    /// Updates the per-thread progress bar every 5MiB written, and sends
    /// byte counts to the main thread via main_pb_tx for total aggregation.
    fn wbfp_p_work(
        mut pack_man: ManagerSync,
        pb: ProgressBar,
        files_list: ConcurrentRing<(PathBuf, FileInfo)>,
        in_dir_path: &Path,
        ff_end: &Arc<AtomicBool>,
        main_pb_tx: &Sender<(u64, u64)>,
        write_optimization: bool,
    ) {
        let mut run_buf = vec![0u8; 1024 * 1024];
        let mut thread_write_len = 0;
        let mut last_thread_write_len = 0;
        let mut update_pb = |write_len,
                             write_len_add,
                             info: &FileInfo,
                             this_in_path: &Path,
                             this_pack_path: &Path| {
            thread_write_len += write_len_add;
            if thread_write_len - last_thread_write_len >= 5 * 1024 * 1024 {
                pb.set_length(info.length());
                pb.set_position(write_len);
                pb.set_message(format!(
                    "\tFile path: {}\n\tPack path: {}",
                    this_in_path.display(),
                    this_pack_path.display()
                ));
                last_thread_write_len = thread_write_len;
            }
        };

        'write_pack: loop {
            let (file_path, file_info) = if let Some(v) = files_list.pop() {
                v
            } else {
                //判断搜索是否结束, 确定是否退出线程
                if ff_end.load(Ordering::Acquire) {
                    break 'write_pack;
                }
                //设置进度条样式
                //AI=== 非 panic 样式构造 / non-panicking style construction
                pb.set_style(progress_style(SPINNER_TEMPLATE));
                //AI_END===
                pb.set_message("已挂起");
                let r = match files_list.pop_wait() {
                    Ok(v) => v,
                    Err(e) => {
                        if !matches!(
                            e.kind,
                            ConcurrentRingErrorKind::WritersClosed
                                | ConcurrentRingErrorKind::AllWaiting
                                | ConcurrentRingErrorKind::SingleInstance
                        ) {
                            error!("队列等待文件信息时发生错误, err: {e}");
                        }
                        break;
                    }
                };
                //恢复进度条样式
                //AI=== 非 panic 样式构造 / non-panicking style construction
                pb.set_style(progress_style(PACK_PROGRESS_STYLE_TEMPLATE).progress_chars("=>-"));
                //AI_END===
                r
            };
            //写入文件前处理
            //路径处理
            //AI=== 去除输入目录前缀失败时记录错误并跳过该文件，不再 panic
            // Log and skip the file when stripping the input-dir prefix fails (no panic).
            let Some(pack_path) = PathTool::path_remove_head(file_path.as_path(), in_dir_path)
            else {
                error!(
                    r#"无法从路径"{}"中去除输入目录前缀"{}"，将跳过该文件"#,
                    file_path.display(),
                    in_dir_path.display()
                );
                let _ = main_pb_tx.send((1, 0)); //仍然计入文件数量 / still count the file
                continue 'write_pack;
            };
            //AI_END===
            let _ = common::copy_file_into_pack(
                &mut pack_man,
                &mut run_buf,
                &file_info,
                &file_path,
                &pack_path,
                Some(main_pb_tx),
                &mut update_pb,
                write_optimization,
            );
            let _ = main_pb_tx.send((1, 0)); //更新总进度条的文件数量
        }
    }

    /// 解包水球包文件到磁盘目录（多线程边发现边解包）。
    ///
    /// 参数: `<包文件路径> <输出目录> [-t <线程数>]`
    ///
    /// 所有线程共用一个工作队列，初始填入根路径名。每个线程从队列取路径，
    /// 按需加载结构/元数据判定类型：
    /// - 目录 → 创建磁盘目录 + 子路径推回队列
    /// - 文件 → 从包虚拟文件系统读取并写入磁盘
    ///
    /// 不使用 `load_all_data`——结构和元数据均在遍历过程中按需加载。
    ///
    /// Multi-threaded unpack (discover-as-you-unpack) using a unified work queue.
    ///
    /// Args: `<pack_path> [output_dir] [-t <thread_count>]`
    ///
    /// All threads share one work queue seeded with root path names. Each thread
    /// pops a path and determines type via on-demand struct/metadata loading:
    /// - Directory → creates disk directory + pushes children back to queue
    /// - File → reads from pack virtual FS and writes to disk
    ///
    /// No `load_all_data` — structures and metadata are loaded on demand during traversal.
    pub fn wbfp_u(
        args: &WaterBallFilePackCommandsUnpack,
        mp: MultiProgress,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let pack_path = &args.pack_path;
        let out_dir_path = common::unpack_out_dir(pack_path, args.out_dir.as_deref())?;
        let thread_count = common::get_thread_count(args.thread_count);

        info!("开始准备解包");
        info!("打开包文件");
        //AI=== 打开包文件/创建输出目录失败向上传播错误，不再 panic
        // Propagate open / create-dir errors instead of panicking.
        let mut pack = ManagerSync::open(pack_path)
            .map_err(|e| Error::other(format!("打开包文件错误 / failed to open pack file: {e}")))?;
        info!("开始复制数据");
        fs::create_dir_all(&out_dir_path)?;
        //AI_END===

        let out_dir_path: Arc<PathBuf> = Arc::new(out_dir_path);
        let root_name_list = pack.get_root_struct_item_name_list()?;
        let attribute = pack.get_manifest_attribute()?;
        let all_file_count = attribute.file_count();
        let data_len = attribute.data_len();

        // 总进度条 / Main progress bar
        let main_pb = create_pb(&mp);
        main_pb.set_length(data_len);
        main_pb.set_style(
            ProgressStyle::default_bar()
                .template(PACK_PROGRESS_STYLE_TEMPLATE)?
                .progress_chars("=>-"),
        );
        main_pb.set_prefix("总进度");

        let pending = Arc::new(AtomicUsize::new(root_name_list.len()));
        let all_done = Arc::new(AtomicBool::new(false));

        // 创建子线程进度条 / Create per-worker progress bars
        let mut worker_pbs = Vec::with_capacity(thread_count);
        for i in 0..thread_count {
            let pb = create_pb(&mp);
            pb.set_style(
                ProgressStyle::default_bar()
                    .template(PACK_PROGRESS_STYLE_TEMPLATE)?
                    .progress_chars("=>-"),
            );
            pb.set_prefix(format!("线程{i}"));
            worker_pbs.push(pb);
        }

        let (handles, rx) = {
            // 统一工作队列
            let queue = ConcurrentRing::new();
            let (tx, rx) = mpsc::channel();
            let mut handles = Vec::with_capacity(thread_count);
            for name in &root_name_list {
                let _ = queue.push(PathBuf::from(name));
            }

            for _ in 0..thread_count {
                //AI=== 解包工作线程创建失败向上传播（原 thread::spawn 会 panic）
                // Propagate unpack-worker spawn failures (thread::spawn would panic).
                Self::wpfp_u_work(
                    &pack,
                    queue.clone(),
                    &pending,
                    &all_done,
                    &tx,
                    &mut worker_pbs,
                    &out_dir_path,
                    &mut handles,
                    args.hash_verify,
                )?;
                //AI_END===
            }
            (handles, rx)
        };

        // 主线程：汇总进度条 / Main thread: aggregate progress
        let mut written_len = 0u64;
        let mut file_count = 0u64;
        let mut last_pb_update_len = 0u64;

        for bytes_written in rx {
            written_len += bytes_written;
            file_count += 1;

            if written_len - last_pb_update_len >= 10 * 1024 * 1024 {
                main_pb.set_position(written_len);
                main_pb.set_message(format!("[{file_count}/{all_file_count}个文件]"));
                last_pb_update_len = written_len;
            }
        }

        //AI=== 解包线程 join 失败转为 io 错误传播 / propagate unpack-thread join failure
        for h in handles {
            h.join()
                .map_err(|_| Error::other("解包线程异常退出 / unpack thread panicked"))??;
        }
        //AI_END===

        info!("操作已完成,文件保存到目录{}", out_dir_path.display());
        Ok(())
    }

    //AI=== 线程创建失败必须返回错误：返回 io::Result<()> 并由调用方用 ? 传播
    // Thread creation failure MUST be returned: return io::Result<()> and let the caller propagate with `?`.
    fn wpfp_u_work(
        pack: &ManagerSync,
        queue: ConcurrentRing<PathBuf>,
        pending: &Arc<AtomicUsize>,
        all_done: &Arc<AtomicBool>,
        tx: &Sender<u64>,
        worker_pbs: &mut Vec<ProgressBar>,
        out_dir_path: &Arc<PathBuf>,
        handles: &mut Vec<JoinHandle<Result<(), Error>>>,
        hash_verify: bool,
    ) -> io::Result<()> {
        //AI_END===
        use std::thread;
        let mut pack = pack.clone();
        let queue = queue.clone();
        let pending = pending.clone();
        let all_done = all_done.clone();
        let tx = tx.clone();
        //AI=== 逻辑错误：解包工作线程数量与进度条数量一致，pop 失败说明不变量被破坏
        // 逻辑错误：每个解包工作线程在派发前都已创建对应进度条，`pop` 必然成功。
        // Logic error: one progress bar is created per unpack worker, so `pop` must succeed.
        let Some(pb) = worker_pbs.pop() else {
            unreachable!(
                "逻辑错误：解包工作线程缺少进度条 / logic error: unpack worker has no progress bar"
            );
        };
        //AI_END===
        let out_dir_path = out_dir_path.to_path_buf();

        //AI=== 线程创建失败必须返回错误：Builder + ? 传播，禁止会 panic 的 thread::spawn
        // Thread creation failure MUST be returned: Builder + `?`; the panicking thread::spawn is forbidden.
        let worker = move || -> io::Result<()> {
            let mut run_buf = vec![0u8; BUF_LEN];

            loop {
                let Some(path) = Self::get_queue_path(&queue, &all_done, &pb) else {
                    return Ok(());
                };

                let item = match pack.get_pack_struct_item(&path) {
                    Ok(v) => v,
                    Err(err) => {
                        error!(r#"无法获取虚拟路径"{}"结构项, err:{err}"#, path.display());
                        if pending.fetch_sub(1, Ordering::Release) == 1 {
                            all_done.store(true, Ordering::Release);
                        }
                        continue;
                    }
                };

                match item.item_type() {
                    PackStructItemType::Dir { .. } => {
                        // 创建磁盘目录 / Create disk directory
                        let out_dir = out_dir_path.join(&path);
                        if let Err(err) = fs::create_dir_all(&out_dir) {
                            error!(r#"无法创建目录"{}", err:{err}"#, out_dir.display());
                        }

                        let children = match pack.get_struct_item_name_list(&path) {
                            Ok(v) => v,
                            Err(err) => {
                                error!(
                                    r#"无法获取虚拟路径"{}"的子项名称, err:{err}"#,
                                    path.display()
                                );
                                if pending.fetch_sub(1, Ordering::Release) == 1 {
                                    all_done.store(true, Ordering::Release);
                                }
                                continue;
                            }
                        };

                        let n = children.len();
                        if n > 0 {
                            pending.fetch_add(n, Ordering::Release);
                            for child in children {
                                let _ = queue.push(path.join(child));
                            }
                        }
                        if pending.fetch_sub(1, Ordering::Release) == 1 {
                            all_done.store(true, Ordering::Release);
                        }
                    }
                    PackStructItemType::File { .. } => {
                        let out_path = out_dir_path.join(&path);
                        let out_path_str = out_path.display().to_string();

                        //解包进度闭包（两分支共用）：消息标明正在解包
                        //Shared extract progress closure: message marks the extract stage
                        let extract_progress = |done: u64, total: u64| {
                            pb.set_length(total);
                            pb.set_position(done);
                            pb.set_message(format!("正在解包: {out_path_str}"));
                        };
                        let bytes_written = if hash_verify {
                            //哈希校验可选（-H/--hash-verify）：指定时先校验再解包，
                            //校验失败/错误则警告并跳过（不写磁盘）
                            //Optional hash verify (-H/--hash-verify): verify first, then
                            //extract; warn and skip on mismatch/error.
                            let mut rw = match pack.open_virtual_file(&path) {
                                Ok(rw) => rw,
                                Err(err) => {
                                    error!("无法打开虚拟文件\"{}\", err: {err:?}", path.display());
                                    let _ = tx.send(0u64);
                                    if pending.fetch_sub(1, Ordering::Release) == 1 {
                                        all_done.store(true, Ordering::Release);
                                    }
                                    continue;
                                }
                            };
                            let hash_ok = match rw.verify_hash(Some(
                                &(|done, total| {
                                    pb.set_length(total);
                                    pb.set_position(done);
                                    pb.set_message(format!("正在哈希校验: {out_path_str}"));
                                }),
                            )) {
                                Ok(ok) => ok,
                                Err(err) => {
                                    warn!(
                                        "虚拟文件\"{}\"哈希验证发生错误, err: {err:?}, 将跳过解包",
                                        path.display()
                                    );
                                    false
                                }
                            };
                            if hash_ok {
                                common::extract_pack_file_to_disk(
                                    &mut pack,
                                    &path,
                                    &out_path,
                                    &mut run_buf,
                                    Some(&extract_progress),
                                )
                            } else {
                                warn!("虚拟文件\"{}\"哈希校验未通过，将跳过解包", path.display());
                                Ok(0)
                            }
                        } else {
                            //未指定哈希校验：直接解包
                            //Without hash verify: extract directly
                            common::extract_pack_file_to_disk(
                                &mut pack,
                                &path,
                                &out_path,
                                &mut run_buf,
                                Some(&extract_progress),
                            )
                        };

                        let _ = tx.send(bytes_written.unwrap_or(0));

                        if pending.fetch_sub(1, Ordering::Release) == 1 {
                            all_done.store(true, Ordering::Release);
                        }
                    }
                }
            }
        };
        //工作线程编号取自已派发句柄数，避免新增参数 / worker index comes from the dispatched handle count, avoiding a new argument
        let worker_index = handles.len();
        handles.push(
            thread::Builder::new()
                .name(format!("wbfp-unpack-worker-{worker_index}"))
                .spawn(worker)?,
        );
        Ok(())
        //AI_END===
    }

    fn get_queue_path(
        queue: &ConcurrentRing<PathBuf>,
        all_done: &Arc<AtomicBool>,
        pb: &ProgressBar,
    ) -> Option<PathBuf> {
        Some(if let Some(p) = queue.pop() {
            p
        } else {
            if all_done.load(Ordering::Acquire) {
                return None;
            }
            //AI=== 非 panic 样式构造 / non-panicking style construction
            pb.set_style(progress_style(SPINNER_TEMPLATE));
            //AI_END===
            pb.set_message("已挂起");
            let r = match queue.pop_wait() {
                Ok(p) => p,
                Err(e) => {
                    if !matches!(
                        e.kind,
                        ConcurrentRingErrorKind::WritersClosed
                            | ConcurrentRingErrorKind::AllWaiting
                            | ConcurrentRingErrorKind::SingleInstance
                    ) {
                        error!("等待路径队列时发生错误, err: {e}");
                    }
                    return None;
                }
            };
            //AI=== 非 panic 样式构造 / non-panicking style construction
            pb.set_style(progress_style(PACK_PROGRESS_STYLE_TEMPLATE).progress_chars("=>-"));
            //AI_END===
            r
        })
    }

    /// 对整个包文件进行 BLAKE3 哈希完整性校验（边发现边校验）。
    ///
    /// 参数: `<包文件路径> [-t <线程数>]`
    ///
    /// 打开包文件后，所有线程共用一个工作队列。初始填入根路径名，每个线程
    /// 从队列取路径，按需加载结构/元数据判定类型：
    /// - 目录 → 子路径推回队列
    /// - 文件 → BLAKE3 校验
    ///
    /// 不使用 `load_all_data` 预加载——结构和元数据均在遍历过程中按需加载。
    /// 校验失败以 `warn` 日志输出，无 warn/error 即表示全部通过。
    ///
    /// Performs BLAKE3 hash integrity verification on the entire pack file
    /// (discover-as-you-verify) using a unified work queue.
    ///
    /// Args: `<pack_path> [-t <thread_count>]`
    ///
    /// After opening the pack, all threads share one work queue seeded with root
    /// paths. Each thread pops a path, resolves its type via on-demand struct/metadata
    /// loading: directories have their children pushed back to the queue; files are
    /// hash-verified. No `load_all_data` pre-loading — structures and metadata are
    /// loaded on demand during traversal. Failures are reported as `warn` logs;
    /// absence of any warn/error output indicates all files passed.
    fn wbfp_h(
        args: &WaterBallFilePackCommandsHashVerify,
        mp: MultiProgress,
    ) -> Result<(), Box<dyn std::error::Error>> {
        //AI=== 复用统一的线程数解析，移除此处线程数强制取值的 panic 路径
        // Reuse the shared thread-count resolution; removes the panicking fallback here.
        let pack_path = &args.pack_path;
        let thread_count = common::get_thread_count(args.thread_count);
        //AI_END===

        info!("开始准备哈希校验");
        info!("打开包文件");
        let mut pack = ManagerSync::open(pack_path)
            .map_err(|e| PackFileError::Other(format!("打开包文件错误: {e}")))?;
        info!("开始哈希校验");
        Self::verify_hash(&mut pack, &mp, thread_count)?;
        info!("操作已完成，没有警告（WARN）或错误（ERROR）说明全部通过。");
        Ok(())
    }

    /// 使用统一工作队列进行多线程并发哈希校验（边发现边校验）。
    ///
    /// 所有线程共用一个工作队列，初始填入根路径名。每个线程从队列取路径，
    /// 通过 `get_pack_struct_item` 判定文件/目录类型（元数据按需加载）：
    /// - 目录 → 将子路径推回队列，唤醒其他线程参与遍历
    /// - 文件 → 调用 `verify_hash()` 校验并通过 mpsc 报告结果
    ///
    /// 使用 `AtomicUsize` 待处理计数控制终止：初始为根路径数，每推进一个子项递增，
    /// 每处理完一个项（目录展开或文件校验）递减。归零时所有线程退出。
    ///
    /// 不使用 `load_all_data`——所有结构和元数据均按需加载，实现真正的
    /// 边发现边校验。
    ///
    /// Multi-threaded hash verification using a unified work queue
    /// (discover-as-you-verify).
    ///
    /// All threads share one work queue seeded with root path names. Each thread
    /// pops a path and determines file/directory type via `get_pack_struct_item`
    /// (metadata loaded on demand):
    /// - Directory → pushes children back to the queue, wakes other threads
    /// - File → calls `verify_hash()` and reports result via mpsc
    ///
    /// An `AtomicUsize` pending counter controls termination: initialized to the
    /// number of root entries, incremented when children are pushed, decremented
    /// when each item (dir expansion or file verify) completes. All threads exit
    /// when the counter reaches zero.
    ///
    /// No `load_all_data` — all structures and metadata are loaded on demand,
    /// achieving true discover-as-you-verify concurrency.
    fn verify_hash(
        pack: &mut ManagerSync,
        mp: &MultiProgress,
        thread_count: usize,
    ) -> io::Result<()> {
        use std::sync::atomic::{AtomicBool, AtomicUsize};

        let root_name_list = pack.get_root_struct_item_name_list()?;
        let attribute = pack.get_manifest_attribute()?;
        let all_file_count = attribute.file_count();
        let data_len = attribute.data_len();

        // 总进度条 / Main progress bar
        let main_pb = create_pb(mp);

        main_pb.set_length(data_len);
        //AI=== 非 panic 样式构造 / non-panicking style construction
        main_pb.set_style(progress_style(PACK_PROGRESS_STYLE_TEMPLATE).progress_chars("=>-"));
        //AI_END===
        main_pb.set_prefix("总进度");

        let pending = Arc::new(AtomicUsize::new(root_name_list.len()));
        let all_done = Arc::new(AtomicBool::new(root_name_list.is_empty()));

        // 创建子线程进度条 / Create per-worker progress bars
        let mut worker_pbs = Vec::with_capacity(thread_count);
        for i in 0..thread_count {
            let pb = create_pb(mp);
            //AI=== 非 panic 样式构造 / non-panicking style construction
            pb.set_style(progress_style(PACK_PROGRESS_STYLE_TEMPLATE).progress_chars("=>-"));
            //AI_END===
            pb.set_prefix(format!("线程{i} "));
            worker_pbs.push(pb);
        }

        let (handles, rx) = {
            // 统一工作队列
            let queue = ConcurrentRing::new();
            let (tx, rx) = mpsc::channel();
            let mut handles = Vec::with_capacity(thread_count);
            for name in &root_name_list {
                let _ = queue.push(PathBuf::from(name));
            }
            for _ in 0..thread_count {
                //AI=== 哈希校验工作线程创建失败向上传播（原 thread::spawn 会 panic）
                // Propagate verify-worker spawn failures (thread::spawn would panic).
                Self::wpfp_h_work(
                    pack,
                    queue.clone(),
                    &pending,
                    &all_done,
                    &tx,
                    &mut worker_pbs,
                    &mut handles,
                )?;
                //AI_END===
            }
            (handles, rx)
        };

        // 主线程：汇总进度条和结果 / Main thread: aggregate progress + results
        let mut verified_len = 0u64;
        let mut verified_count = 0u64;
        let mut last_pb_update_len = 0u64;
        let mut filed_count = 0;

        for (path_str, file_len, result) in rx {
            verified_len += file_len;
            verified_count += 1;

            // 限频更新：每 10 MiB 更新一次
            // Rate-limit main bar: update every 10 MiB
            if verified_len - last_pb_update_len >= 10 * 1024 * 1024 {
                main_pb.set_position(verified_len);
                main_pb.set_message(format!("[{verified_count}/{all_file_count}个文件]"));
                last_pb_update_len = verified_len;
            }

            if let Some(ok) = result
                && !ok
            {
                filed_count += 1;
                warn!(r#"[{filed_count}]虚拟文件 "{path_str}" 哈希校验未通过"#);
            }
        }

        //AI=== 哈希校验线程 join 失败转为 io 错误传播 / propagate verify-thread join failure
        for h in handles {
            h.join()
                .map_err(|_| Error::other("哈希校验线程异常退出 / verify thread panicked"))??;
        }
        //AI_END===

        Ok(())
    }

    //AI=== 线程创建失败必须返回错误：返回 io::Result<()> 并由调用方用 ? 传播
    // Thread creation failure MUST be returned: return io::Result<()> and let the caller propagate with `?`.
    fn wpfp_h_work(
        pack: &ManagerSync,
        queue: ConcurrentRing<PathBuf>,
        pending: &Arc<AtomicUsize>,
        all_done: &Arc<AtomicBool>,
        tx: &Sender<(String, u64, Option<bool>)>,
        worker_pbs: &mut Vec<ProgressBar>,
        handles: &mut Vec<JoinHandle<Result<(), Error>>>,
    ) -> io::Result<()> {
        //AI_END===
        use std::thread;
        let mut pack = pack.clone();
        let pending = pending.clone();
        let all_done = all_done.clone();
        let tx = tx.clone();
        //AI=== 逻辑错误：哈希校验工作线程数量与进度条数量一致，pop 失败说明不变量被破坏
        // 逻辑错误：每个哈希校验工作线程在派发前都已创建对应进度条，`pop` 必然成功。
        // Logic error: one progress bar is created per verify worker, so `pop` must succeed.
        let Some(pb) = worker_pbs.pop() else {
            unreachable!(
                "逻辑错误：哈希校验工作线程缺少进度条 / logic error: verify worker has no progress bar"
            );
        };
        //AI_END===

        //AI=== 线程创建失败必须返回错误：Builder + ? 传播，禁止会 panic 的 thread::spawn
        // Thread creation failure MUST be returned: Builder + `?`; the panicking thread::spawn is forbidden.
        let worker = move || -> io::Result<()> {
            loop {
                // 1. 从队列取路径（空队列时 Condvar 挂起）
                //    Pop path from queue (Condvar suspend when empty)
                let Some(path) = Self::get_queue_path(&queue, &all_done, &pb) else {
                    return Ok(());
                };

                // 2. 判定路径类型（按需加载目录结构或文件元数据）
                //    Determine path type (lazy-load dir struct or file metadata)
                let item = match pack.get_pack_struct_item(&path) {
                    Ok(v) => v,
                    Err(err) => {
                        error!(r#"无法获取虚拟路径"{}"结构项, err:{err}"#, path.display());
                        if pending.fetch_sub(1, Ordering::Release) == 1 {
                            all_done.store(true, Ordering::Release);
                        }
                        continue;
                    }
                };

                match item.item_type() {
                    PackStructItemType::Dir { .. } => {
                        // 展开目录：获取子项并推入队列
                        // Expand directory: get children and push to queue
                        let children = match pack.get_struct_item_name_list(&path) {
                            Ok(v) => v,
                            Err(err) => {
                                error!(
                                    r#"无法获取虚拟路径"{}"的子项名称, err:{err}"#,
                                    path.display()
                                );
                                if pending.fetch_sub(1, Ordering::Release) == 1 {
                                    all_done.store(true, Ordering::Release);
                                }
                                continue;
                            }
                        };

                        let n = children.len();
                        if n > 0 {
                            pending.fetch_add(n, Ordering::Release);
                            for child in children {
                                let _ = queue.push(path.join(child));
                            }
                        }
                        // 目录自身处理完毕 / Directory itself resolved
                        if pending.fetch_sub(1, Ordering::Release) == 1 {
                            all_done.store(true, Ordering::Release);
                        }
                    }
                    PackStructItemType::File { .. } => {
                        let mut rw = match pack.open_virtual_file(&path) {
                            Ok(v) => v,
                            Err(err) => {
                                error!("无法获取包文件读写器，err: {err}");
                                let _ = tx.send((path.display().to_string(), 0u64, None));
                                if pending.fetch_sub(1, Ordering::Release) == 1 {
                                    all_done.store(true, Ordering::Release);
                                }
                                continue;
                            }
                        };

                        let file_len = rw.get_len();

                        let path_str = path.display().to_string();
                        let result = match rw.verify_hash(Some(
                            &(|done, total| {
                                pb.set_length(total);
                                pb.set_position(done);
                                pb.set_message(format!("\tFile path: {path_str}"));
                            }),
                        )) {
                            Ok(true) => Some(true),
                            Ok(false) => Some(false),
                            Err(err) => {
                                warn!(
                                    r#"虚拟文件"{}"哈希验证发生错误, err: {err:?}"#,
                                    path.display()
                                );
                                None
                            }
                        };

                        let _ = tx.send((path.display().to_string(), file_len, result));
                        // 文件处理完毕 / File resolved
                        if pending.fetch_sub(1, Ordering::Release) == 1 {
                            all_done.store(true, Ordering::Release);
                        }
                    }
                }
            }
        };
        //工作线程编号取自已派发句柄数，避免新增参数 / worker index comes from the dispatched handle count, avoiding a new argument
        let worker_index = handles.len();
        handles.push(
            thread::Builder::new()
                .name(format!("wbfp-hash-verify-worker-{worker_index}"))
                .spawn(worker)?,
        );
        Ok(())
        //AI_END===
    }
}
//AI_END===
