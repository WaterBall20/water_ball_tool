//! wbfp 子命令模块 / wbfp subcommand module
//!
//! 实现水球包文件（WBFP）的打包、解包和哈希校验逻辑。
//! 支持多线程写入（生产者-消费者模式，通过 `FileFinder::search_stream` 流式发现文件）、
//! 每个工作线程独立的进度条显示，以及大文件（>512MiB）日志提示。
//!
//! Implements pack, unpack, and hash-verify logic for the WaterBall File Pack (WBFP).
//! Supports multi-threaded writing (producer-consumer pattern via `FileFinder::search_stream`
//! for streaming file discovery), per-worker-thread progress bars, and large file (>512MiB) log notices.

use crate::command::{BUF_LEN, PACK_PROGRESS_STYLE_TEMPLATE, SPINNER_TEMPLATE, create_pb};
use clap::error::Result;
use clap::{Args, Subcommand};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::collections::VecDeque;
use std::fs::File;
use std::io::{Error, ErrorKind, Read, Write};
use std::num::NonZero;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread::JoinHandle;
use std::{fs, io};
use tracing::{error, info, warn};
use water_ball_tool::file_finder::{FileFinder, FileInfo, FileKind, SearchEvent};
use water_ball_tool::tools::PathTool;
use water_ball_tool::wb_files_pack::manager_sync::ManagerSync;
use water_ball_tool::wb_files_pack::{PackFileError, PackStructItemType};
type ArcMutex<T> = Arc<Mutex<T>>;

/// wbfp 命令参数 / wbfp command arguments
#[derive(Args, Debug)]
pub(crate) struct WaterBallFilePackCommand {
    #[command(subcommand)]
    commands: WaterBallFilePackCommands,
}

impl WaterBallFilePackCommand {
    #[cfg(test)]
    pub(crate) fn new(commands: WaterBallFilePackCommands) -> Self {
        Self { commands }
    }
}

/// wbfp 子命令枚举 / wbfp subcommand enum
#[derive(Subcommand, Debug)]
pub(crate) enum WaterBallFilePackCommands {
    /// 解包 / Unpack
    #[command(visible_alias = "u")]
    Unpack(WaterBallFilePackCommandsUnpack),
    /// 打包 / Pack
    #[command(visible_alias = "p")]
    Pack(WaterBallFilePackCommandsPack),
    /// 对整个包文件哈希校验 / Hash verify the entire pack file
    #[command(visible_alias = "h")]
    HashVerify(WaterBallFilePackCommandsHashVerify),
}
/// 解包子命令参数 / Unpack subcommand arguments
#[derive(Args, Debug)]
pub(crate) struct WaterBallFilePackCommandsUnpack {
    /// 水球包包文件路径 / WaterBall pack file path
    pack_path: String,
    /// 输出目录路径（缺省则去掉 .wbfp 后缀，生成在同级目录）/ Output directory path (defaults to stripping .wbfp suffix, placed next to pack file)
    out_dir: Option<String>,
    /// 解包前进行哈希校验 / Hash verify before unpacking
    #[arg(short = 'H', long)]
    hash_verify: bool,
    /// 解包时所用的线程数，缺省使用CPU线程数量 / Thread count for unpacking, defaults to CPU count
    #[arg(short, long)]
    thread_count: Option<usize>,
}
impl WaterBallFilePackCommandsUnpack {
    #[cfg(test)]
    pub(crate) fn new(
        pack_path: String,
        out_dir: Option<String>,
        hash_verify: bool,
        thread_count: Option<usize>,
    ) -> Self {
        Self {
            pack_path,
            out_dir,
            hash_verify,
            thread_count,
        }
    }
}

/// 打包子命令参数 / Pack subcommand arguments
#[derive(Args, Debug)]
pub(crate) struct WaterBallFilePackCommandsPack {
    /// 打包的文件或目录路径 / Input file or directory path
    in_path: String,
    /// 输出的包文件路径（省略则在当前目录生成同名文件）/ Output pack file path (generates same-name file in CWD if omitted)
    out_pack_path: Option<String>,
    /// 不分离清单到 .wbm 文件 / Do not separate manifest to .wbm file
    #[arg(short, long)]
    no_separation: bool,
    /// 复制时所用的线程数，缺省使用CPU线程数量
    #[arg(short, long)]
    thread_count: Option<usize>,
    /// 不启用写入优化
    #[arg(short = 'w', long)]
    no_write_optimization: bool,
}
impl WaterBallFilePackCommandsPack {
    #[cfg(test)]
    pub(crate) fn new(
        in_path: String,
        out_pack_path: Option<String>,
        no_separation: bool,
        thread_count: Option<usize>,
        no_write_optimization: bool,
    ) -> Self {
        Self {
            in_path,
            out_pack_path,
            no_separation,
            thread_count,
            no_write_optimization,
        }
    }
}
/// 哈希校验子命令参数 / Hash verify subcommand arguments
#[derive(Args, Debug)]
pub(crate) struct WaterBallFilePackCommandsHashVerify {
    /// 包文件路径 / Pack file path
    pack_path: String,
    /// 校验时所用的线程数，缺省使用CPU线程数量 / Thread count for verification, defaults to CPU count
    #[arg(short, long)]
    thread_count: Option<usize>,
    #[arg(short, long)]
    verbose: bool,
}
impl WaterBallFilePackCommandsHashVerify {
    #[cfg(test)]
    pub(crate) fn new(pack_path: String, verbose: bool) -> Self {
        Self {
            pack_path,
            thread_count: None,
            verbose,
        }
    }
}
/// wbfp 命令路由入口，根据子命令分发到对应的执行函数。
/// 每个执行函数独立处理进度条和多线程逻辑。
///
/// wbfp command router. Dispatches to the appropriate execution function
/// based on the subcommand. Each function handles its own progress bars
/// and multi-threading logic independently.
pub fn wbfp(
    args: &WaterBallFilePackCommand,
    mp: Option<&MultiProgress>,
) -> Result<(), Box<dyn std::error::Error>> {
    let args2 = &args.commands;
    match args2 {
        WaterBallFilePackCommands::Unpack(u) => WaterBallFilePackArgsRuning::wbfp_u(u, mp),
        WaterBallFilePackCommands::Pack(p) => WaterBallFilePackArgsRuning::wbfp_p(p, mp),

        WaterBallFilePackCommands::HashVerify(h) => WaterBallFilePackArgsRuning::wbfp_h(h, mp),
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
        mp: Option<&MultiProgress>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        //源目录路径
        let in_path = PathBuf::from(&args.in_path);
        //输出的包文件路径
        let mut pack_path = {
            if let Some(pack_path) = &args.out_pack_path {
                //存在路径参数
                //转换为路径并判断是否存在且是目录
                let pack_path: &Path = pack_path.as_ref();
                if pack_path.exists() && pack_path.is_dir() {
                    //在目录创建同名前缀文件
                    if let Some(name) = in_path.file_name() {
                        pack_path.join(name)
                    } else {
                        //如果没有文件名就直接使用参数
                        let msg = "提供的输入路径是目录，且无法获取输入路径文件名，无法自动设置输出文件路径";
                        error!(msg);
                        Err(Error::other(msg))?
                    }
                } else {
                    pack_path.to_path_buf()
                }
            } else {
                //不存在路径参数
                //在工作目录创建同名前缀
                let pack_pach = PathBuf::from(".");
                if let Some(name) = in_path.file_name() {
                    pack_pach.join(name)
                } else {
                    let msg =
                        "未提供输出路径，且无法获取输入路径文件名，无法自动设置输出文件路径。";
                    error!(msg);
                    Err(Error::other(msg))?
                }
            }
        };
        //确保输出文件名
        pack_path.set_extension("wbfp");
        //分离数据文件
        let separate_manifest = !args.no_separation;
        //写入优化
        let write_optimization = !args.no_write_optimization;
        //搜索进度条
        let ff_pb = create_pb(mp);

        //包文件进度条
        let wb_pb = create_pb(mp).map(|pb| Arc::new(Mutex::new(pb)));

        info!("开始准备打包");

        let data_len = Arc::new(Mutex::new(0));

        let pack = {
            if pack_path.exists() {
                info!("包文件已存在，将打开并修改");
                if write_optimization {
                    info!("已启用写入优化，未更改的文件将跳过");
                }
                ManagerSync::options()
                    .read(true)
                    .write(true)
                    .open(&pack_path)
                    .expect("打开包文件错误")
            } else {
                info!("创建新包文件并初始化");
                ManagerSync::options()
                    .write(true)
                    .create_new(true)
                    .separate_manifest(separate_manifest)
                    .open(&pack_path)
                    .expect("创建包文件错误")
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
            mp,
            &pack_path,
        )
    }

    fn wbfp_p_run(
        wb_pb: Option<ArcMutex<ProgressBar>>,
        data_len: ArcMutex<u64>,
        in_path: &Path,
        ff_pb: Option<ProgressBar>,
        mut pack: ManagerSync,
        write_optimization: bool,
        args: &WaterBallFilePackCommandsPack,
        mp: Option<&MultiProgress>,
        pack_path: &Path,
    ) -> Result<(), Box<dyn std::error::Error>> {
        use std::thread;
        //复制操作===
        info!("开始复制数据");
        //设置为包文件具体进度条
        if let Some(pb) = &wb_pb {
            let pb = pb.lock().unwrap();
            pb.set_length(*data_len.lock().unwrap());
            pb.set_style(
                ProgressStyle::default_bar()
                    .template(PACK_PROGRESS_STYLE_TEMPLATE)
                    .unwrap()
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
            let mut file_name = in_path.file_name().unwrap().to_str().unwrap().to_string();
            file_name.push_str(".wbfp");
            let file_metadata = in_path.metadata()?;
            let file_info = FileInfo::new(
                file_name.clone(),
                file_metadata.len(),
                FileFinder::get_file_modified(&file_metadata),
                FileKind::File,
            );
            let mut run_buf = vec![0u8; 1024 * 1024];
            let mut write_len = 0;
            let mut last_write_len = 0;
            let mut update_pb =
                |write_len_2, _, info: &FileInfo, this_in_path: &Path, this_pack_path: &Path| {
                    write_len = write_len_2;
                    if write_len - last_write_len >= 5 * 1024 * 1024
                        && let Some(pb) = &wb_pb
                    {
                        let pb = pb.lock().unwrap();
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

            Self::copy_file_into_pack(
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
            let thread_count = if let Some(v) = args.thread_count {
                v
            } else {
                let thread_count = thread::available_parallelism()
                    .unwrap_or(NonZero::new(8).unwrap())
                    .get();
                info!("未指定线程数量，将使用{thread_count}线程");
                thread_count
            };

            //搜索文件===
            info!("开始搜索文件");
            warn!("目前搜索将跳过符号链接");
            //搜索结果队列
            let results = Arc::new(Mutex::new(VecDeque::new()));
            //搜索结束标志
            let ff_end = Arc::new(Mutex::new(false));
            //控制线程继续和等待的控制变量
            let condver = Arc::new(Condvar::new());
            //文件计数
            let file_count = Arc::new(Mutex::new(0));

            //创建专门的线程搜索
            let ff_thread = Self::wbfp_p_search(
                in_path,
                &results,
                &ff_end,
                &condver,
                &file_count,
                &data_len,
                ff_pb,
            );

            //复制线程
            let wp_thread = Self::wbfp_p_copy(
                &pack,
                &results,
                in_path,
                &condver,
                &ff_end,
                thread_count,
                mp,
                write_optimization,
                wb_pb,
                data_len,
                file_count,
            );
            //等待线程结束
            if let Err(e) = ff_thread.join().unwrap() {
                error!("文件搜索线程错误：{e}");
            }
            *ff_end.lock().unwrap() = true;
            //唤醒所有线程，避免死锁
            condver.notify_all();
            wp_thread.join().unwrap();
        }
        info!("操作已完成,文件保存到{}", pack_path.display());
        Ok(())
    }

    ///打包搜索线程
    fn wbfp_p_search(
        in_path: &Path,
        results: &ArcMutex<VecDeque<(PathBuf, FileInfo)>>,
        ff_end: &ArcMutex<bool>,
        condver: &Arc<Condvar>,
        file_count: &ArcMutex<u64>,
        data_len: &ArcMutex<u64>,
        ff_pb: Option<ProgressBar>,
    ) -> JoinHandle<Result<(), Error>> {
        use std::thread;

        //创建专门的线程搜索
        {
            let in_dir_path = in_path.to_path_buf();
            let results = results.clone();
            let ff_end = ff_end.clone();
            let condver = condver.clone();
            let file_count = file_count.clone();
            let data_len = data_len.clone();

            thread::spawn(move || -> io::Result<()> {
                let ff = FileFinder;
                let dir_count = Arc::new(Mutex::new(0));
                //搜索进度
                let (tx, rx) = mpsc::channel();
                let (search_stream_handle, stream_results) =
                    ff.search_stream(&in_dir_path, true, tx, 2)?;
                //更新进度线程

                if let Some(ff_pb) = ff_pb {
                    let file_count = file_count.clone();
                    let dir_count = dir_count.clone();
                    thread::spawn(move || {
                        for (add_file, add_dir) in rx {
                            let mut file_count = file_count.lock().unwrap();
                            let mut dir_count = dir_count.lock().unwrap();

                            *file_count += add_file;
                            *dir_count += add_dir;
                            let all_count = *file_count + *dir_count;
                            ff_pb.set_position(all_count);
                            //10的倍数才更新
                            if all_count.is_multiple_of(100) {
                                ff_pb.set_message(format!(
                                    "[搜索文件]已发现 {file_count} 文件和 {dir_count} 个目录"
                                ));
                            }
                        }
                    });
                }

                for event in stream_results {
                    match event {
                        SearchEvent::Entry(path, info) => {
                            //只有文件才会加入队列
                            if let FileKind::File = info.file_kind() {
                                *data_len.lock().unwrap() += info.length();
                                results.lock().unwrap().push_back((path, info));
                                condver.notify_one();
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

                search_stream_handle.join().unwrap()?;
                info!(
                    "文件搜索已完成: {}个文件，{}个目录",
                    *file_count.lock().unwrap(),
                    *dir_count.lock().unwrap()
                );
                *ff_end.lock().unwrap() = true;
                //唤醒所有线程，避免死锁
                condver.notify_all();
                Ok(())
            })
        }
    }

    fn wbfp_p_copy(
        pack: &ManagerSync,
        results: &ArcMutex<VecDeque<(PathBuf, FileInfo)>>,
        in_path: &Path,
        condver: &Arc<Condvar>,
        ff_end: &ArcMutex<bool>,
        thread_count: usize,
        mp: Option<&MultiProgress>,
        write_optimization: bool,
        wb_pb: Option<ArcMutex<ProgressBar>>,
        data_len: ArcMutex<u64>,
        file_count: ArcMutex<u64>,
    ) -> JoinHandle<()> {
        use std::thread;
        let pack = pack.clone();
        let results = results.clone();
        let in_dir_path = in_path.to_path_buf();
        let condver = condver.clone();
        let ff_end = ff_end.clone();

        //创建线程进度条
        let mut wb_pbs = Vec::with_capacity(thread_count);
        for _ in 0..thread_count {
            let pb = create_pb(mp);
            if let Some(pb) = &pb {
                pb.set_style(
                    ProgressStyle::default_bar()
                        .template(PACK_PROGRESS_STYLE_TEMPLATE)
                        .unwrap()
                        .progress_chars("=>-"),
                );
            }
            wb_pbs.push(pb);
        }

        thread::spawn(move || {
            let (tx, rx) = mpsc::channel();
            let mut thread_handle = Vec::with_capacity(thread_count);
            //
            for index in 0..thread_count {
                let main_pb_tx = tx.clone();
                let pack_man = pack.clone();
                let files_list = results.clone();
                let in_dir_path = in_dir_path.clone();
                let condver = condver.clone();
                let pb = wb_pbs.pop().unwrap();
                let ff_end = ff_end.clone();
                if let Some(pb) = &pb {
                    pb.set_prefix(format!("线程{index}"));
                }
                thread_handle.push(thread::spawn(move || {
                    Self::wbfp_p_work(
                        pack_man,
                        pb,
                        files_list,
                        &in_dir_path,
                        ff_end,
                        condver,
                        main_pb_tx,
                        write_optimization,
                    )
                }));
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
                if write_len - last_write_len > 10 * 1024 * 1024
                    && let Some(pb) = &wb_pb
                {
                    let pb = pb.lock().unwrap();
                    //总大小与实际写入量对齐（源文件可能在复制期间增长）
                    let mut total = data_len.lock().unwrap();
                    if write_len > *total {
                        *total = write_len;
                    }
                    pb.set_length(*total);
                    pb.set_position(write_len);
                    pb.set_message(format!(
                        "[{write_file_count}/{}个文件]",
                        file_count.lock().expect("获取文件数量")
                    ));
                    last_write_len = write_len;
                }
            }

            //等待所有工作线程结束
            for (index, item) in thread_handle.into_iter().enumerate() {
                if let Err(err) = item.join().unwrap() {
                    error!("线程{index}，发生错误：{err}");
                }
            }
        })
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
        pb: Option<ProgressBar>,
        files_list: ArcMutex<VecDeque<(PathBuf, FileInfo)>>,
        in_dir_path: &Path,
        ff_end: ArcMutex<bool>,
        condver: Arc<Condvar>,
        main_pb_tx: Sender<(u64, u64)>,
        write_optimization: bool,
    ) -> io::Result<()> {
        let mut run_buf = vec![0u8; 1024 * 1024];
        let mut thread_write_len = 0;
        let mut last_thread_write_len = 0;
        let mut update_pb = |write_len,
                             write_len_add,
                             info: &FileInfo,
                             this_in_path: &Path,
                             this_pack_path: &Path| {
            thread_write_len += write_len_add;
            if thread_write_len - last_thread_write_len >= 5 * 1024 * 1024
                && let Some(pb) = &pb
            {
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
            let (file_path, file_info) = match files_list.lock().unwrap().pop_front() {
                Some(v) => v,
                None => {
                    //进入挂起
                    loop {
                        //判断搜索是否结束, 确定是否退出线程
                        if *ff_end.lock().unwrap() {
                            //唤醒所有线程，避免死锁
                            condver.notify_all();
                            break 'write_pack;
                        }
                        //设置进度条样式
                        if let Some(pb) = &pb {
                            pb.set_style(
                                ProgressStyle::default_bar()
                                    .template(SPINNER_TEMPLATE)
                                    .unwrap(),
                            );
                            pb.set_message("已挂起");
                        }

                        let mut files_list = files_list.lock().unwrap();
                        files_list = condver.wait(files_list).unwrap();

                        if let Some(v) = files_list.pop_front() {
                            //恢复进度条样式
                            if let Some(pb) = &pb {
                                pb.set_style(
                                    ProgressStyle::default_bar()
                                        .template(PACK_PROGRESS_STYLE_TEMPLATE)
                                        .unwrap()
                                        .progress_chars("=>-"),
                                );
                            }
                            break v;
                        }
                    }
                }
            };
            //写入文件前处理
            //路径处理
            let pack_path = PathTool::path_remove_head(file_path.as_path(), in_dir_path).unwrap();
            Self::copy_file_into_pack(
                &mut pack_man,
                &mut run_buf,
                &file_info,
                &file_path,
                &pack_path,
                Some(&main_pb_tx),
                &mut update_pb,
                write_optimization,
            );
            let _ = main_pb_tx.send((1, 0)); //更新总进度条的文件数量
        }
        Ok(())
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
    fn copy_file_into_pack(
        pack_man: &mut ManagerSync,
        run_buf: &mut [u8],
        info: &FileInfo,
        this_in_path: &Path,
        this_pack_path: &Path,
        main_pb_tx: Option<&Sender<(u64, u64)>>,
        update_pb: &mut dyn FnMut(u64, u64, &FileInfo, &Path, &Path),
        write_optimization: bool,
    ) {
        //尝试打开文件
        let mut in_file = match File::open(this_in_path) {
            Ok(file) => file,
            Err(err) => {
                match err.kind() {
                    ErrorKind::PermissionDenied => {
                        //权限不足
                        error!(
                            r#"无法打开文件"{}"，权限不足，将跳过，err:{err}"#,
                            this_in_path.display()
                        );
                    }
                    _ => error!(
                        r#"[未处理错误]无法打开文件"{}"，err:{err}"#,
                        this_in_path.display()
                    ),
                }
                return;
            }
        };
        //尝试创建虚拟文件（已知大小：按 128B 精确分配，避免 4MiB 整块浪费）
        //Try creating the virtual file (known size: exact 128B-aligned allocation)
        let mut out_file = match pack_man
            .virtual_file_options()
            .write(true)
            .create_new(true)
            .alloc_size(Some(info.length()))
            .open(&this_pack_path)
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
                            return;
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
                        return;
                    }
                }
            }
        };
        //写入操作
        //读多少写多少：短写时续写余量不丢弃；源文件搜索后增长也全部写入，
        //虚拟文件实际大小由写入自动同步；EOF（read 返回 0）为正常结束条件。
        let mut write_len = 0;
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
                        break;
                    }
                }
                Err(err) => {
                    error!(
                        r#"读取文件"{}"失败，将跳过，err:{err}"#,
                        this_in_path.display()
                    );
                    break;
                }
            }
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
        mp: Option<&MultiProgress>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        use std::thread;

        let pack_path = &args.pack_path;
        // 智能推导输出路径：若未指定，取文件名去掉 .wbfp 后缀，放在工作目录下
        // Smart output dir: if unspecified, strip .wbfp suffix from filename, place in CWD
        let out_dir_path = if let Some(d) = &args.out_dir {
            PathBuf::from(d)
        } else {
            let stem = Path::new(pack_path).file_stem().ok_or_else(|| {
                Box::<dyn std::error::Error>::from(format!(
                    "无法从包文件路径提取前缀以推导输出目录: {pack_path}"
                ))
            })?;
            let out = Path::new(".").join(stem);
            info!("未指定输出路径，将解包到: {}", out.display());
            out
        };
        let thread_count = args.thread_count.unwrap_or_else(|| {
            thread::available_parallelism()
                .unwrap_or(NonZero::new(8).unwrap())
                .get()
        });

        info!("开始准备解包");
        info!("打开包文件");
        let mut pack = ManagerSync::open(pack_path).expect("打开包文件错误");
        info!("开始复制数据");
        fs::create_dir_all(&out_dir_path).expect("无法创建数据路径");

        let root_name_list = pack.get_root_struct_item_name_list()?;
        let attribute = pack.get_manifest_attribute()?;
        let all_file_count = attribute.file_count();
        let data_len = attribute.data_len();

        // 总进度条 / Main progress bar
        let main_pb = create_pb(mp);
        if let Some(pb) = &main_pb {
            pb.set_length(data_len);
            pb.set_style(
                ProgressStyle::default_bar()
                    .template(PACK_PROGRESS_STYLE_TEMPLATE)
                    .unwrap()
                    .progress_chars("=>-"),
            );
            pb.set_prefix("总进度");
        }

        // 统一工作队列 + 终止控制
        // Unified work queue + termination control
        let queue: Arc<(Mutex<VecDeque<PathBuf>>, Condvar)> =
            Arc::new((Mutex::new(VecDeque::new()), Condvar::new()));
        let pending = Arc::new(AtomicUsize::new(root_name_list.len()));
        let all_done = Arc::new(AtomicBool::new(false));
        {
            let mut q = queue.0.lock().unwrap();
            for name in &root_name_list {
                q.push_back(PathBuf::from(name));
            }
        }

        // 创建子线程进度条 / Create per-worker progress bars
        let mut worker_pbs: Vec<Option<ProgressBar>> = Vec::with_capacity(thread_count);
        for i in 0..thread_count {
            let pb = create_pb(mp);
            if let Some(pb) = &pb {
                pb.set_style(
                    ProgressStyle::default_bar()
                        .template(PACK_PROGRESS_STYLE_TEMPLATE)
                        .unwrap()
                        .progress_chars("=>-"),
                );
                pb.set_prefix(format!("线程{i}"));
            }
            worker_pbs.push(pb);
        }

        let (tx, rx) = mpsc::channel();
        let mut handles = Vec::with_capacity(thread_count);
        let out_dir_path: Arc<PathBuf> = Arc::new(out_dir_path);

        for _ in 0..thread_count {
            Self::wpfp_u_work(
                &pack,
                &queue,
                &pending,
                &all_done,
                &tx,
                &mut worker_pbs,
                &out_dir_path,
                &mut handles,
                args.hash_verify,
            );
        }
        drop(tx);

        // 主线程：汇总进度条 / Main thread: aggregate progress
        let mut written_len = 0u64;
        let mut file_count = 0u64;
        let mut last_pb_update_len = 0u64;

        for bytes_written in rx {
            written_len += bytes_written;
            file_count += 1;

            if written_len - last_pb_update_len >= 10 * 1024 * 1024 {
                if let Some(pb) = &main_pb {
                    pb.set_position(written_len);
                    pb.set_message(format!("[{file_count}/{all_file_count}个文件]"));
                }
                last_pb_update_len = written_len;
            }
        }

        for h in handles {
            h.join().unwrap()?;
        }

        info!("操作已完成,文件保存到目录{}", out_dir_path.display());
        Ok(())
    }

    fn wpfp_u_work(
        pack: &ManagerSync,
        queue: &Arc<(Mutex<VecDeque<PathBuf>>, Condvar)>,
        pending: &Arc<AtomicUsize>,
        all_done: &Arc<AtomicBool>,
        tx: &Sender<u64>,
        worker_pbs: &mut Vec<Option<ProgressBar>>,
        out_dir_path: &Arc<PathBuf>,
        handles: &mut Vec<JoinHandle<Result<(), Error>>>,
        hash_verify: bool,
    ) {
        use std::thread;
        let mut pack = pack.clone();
        let queue = queue.clone();
        let pending = pending.clone();
        let all_done = all_done.clone();
        let tx = tx.clone();
        let w_pb = worker_pbs.pop().unwrap();
        let out_dir_path = out_dir_path.to_path_buf();

        handles.push(thread::spawn(move || -> io::Result<()> {
            let mut run_buf = vec![0u8; BUF_LEN];

            loop {
                let path = {
                    let (lock, cvar) = &*queue;
                    let mut q = lock.lock().unwrap();
                    loop {
                        if let Some(p) = q.pop_front() {
                            break p;
                        }
                        if all_done.load(Ordering::SeqCst) {
                            cvar.notify_all();
                            return Ok(());
                        }
                        if let Some(pb) = &w_pb {
                            pb.set_style(
                                ProgressStyle::default_bar()
                                    .template(SPINNER_TEMPLATE)
                                    .unwrap(),
                            );
                            pb.set_message("已挂起");
                        }
                        q = cvar.wait(q).unwrap();
                        if let Some(pb) = &w_pb {
                            pb.set_style(
                                ProgressStyle::default_bar()
                                    .template(PACK_PROGRESS_STYLE_TEMPLATE)
                                    .unwrap()
                                    .progress_chars("=>-"),
                            );
                        }
                    }
                };

                let item = match pack.get_pack_struct_item(&path) {
                    Ok(v) => v,
                    Err(err) => {
                        error!(r#"无法获取虚拟路径"{}"结构项, err:{err}"#, path.display());
                        if pending.fetch_sub(1, Ordering::SeqCst) == 1 {
                            all_done.store(true, Ordering::SeqCst);
                            queue.1.notify_all();
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
                                if pending.fetch_sub(1, Ordering::SeqCst) == 1 {
                                    all_done.store(true, Ordering::SeqCst);
                                    queue.1.notify_all();
                                }
                                continue;
                            }
                        };

                        let n = children.len();
                        if n > 0 {
                            pending.fetch_add(n, Ordering::SeqCst);
                            let mut q = queue.0.lock().unwrap();
                            for child in children {
                                q.push_back(path.join(child));
                            }
                            queue.1.notify_all();
                        }
                        if pending.fetch_sub(1, Ordering::SeqCst) == 1 {
                            all_done.store(true, Ordering::SeqCst);
                            queue.1.notify_all();
                        }
                    }
                    PackStructItemType::File { .. } => {
                        let out_path = out_dir_path.join(&path);
                        let out_path_str = out_path.display().to_string();

                        //解包进度闭包（两分支共用）：消息标明正在解包
                        //Shared extract progress closure: message marks the extract stage
                        let extract_progress = |done: u64, total: u64| {
                            if let Some(pb) = &w_pb {
                                pb.set_length(total);
                                pb.set_position(done);
                                pb.set_message(format!("正在解包: {out_path_str}"));
                            }
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
                                    if pending.fetch_sub(1, Ordering::SeqCst) == 1 {
                                        all_done.store(true, Ordering::SeqCst);
                                        queue.1.notify_all();
                                    }
                                    continue;
                                }
                            };
                            let hash_ok = match rw.verify_hash(Some(
                                &(|done, total| {
                                    if let Some(pb) = &w_pb {
                                        pb.set_length(total);
                                        pb.set_position(done);
                                        pb.set_message(format!("正在哈希校验: {out_path_str}"));
                                    }
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
                                Self::extract_pack_file_to_disk(
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
                            Self::extract_pack_file_to_disk(
                                &mut pack,
                                &path,
                                &out_path,
                                &mut run_buf,
                                Some(&extract_progress),
                            )
                        };

                        let _ = tx.send(bytes_written.unwrap_or(0));

                        if pending.fetch_sub(1, Ordering::SeqCst) == 1 {
                            all_done.store(true, Ordering::SeqCst);
                            queue.1.notify_all();
                        }
                    }
                }
            }
        }));
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
    fn extract_pack_file_to_disk(
        pack: &mut ManagerSync,
        pack_path: &Path,
        out_path: &Path,
        run_buf: &mut [u8],
        progress: Option<&dyn Fn(u64, u64)>,
    ) -> io::Result<u64> {
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
                water_ball_tool::tools::bytes_len_to_string(file_len),
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
        mp: Option<&MultiProgress>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        use std::thread;

        let pack_path = &args.pack_path;
        let thread_count = args.thread_count.unwrap_or_else(|| {
            thread::available_parallelism()
                .unwrap_or(NonZero::new(8).unwrap())
                .get()
        });

        info!("开始准备哈希校验");
        info!("打开包文件");
        let mut pack = ManagerSync::open(pack_path)
            .map_err(|e| PackFileError::Other(format!("打开包文件错误: {e}")))?;
        info!("开始哈希校验");
        Self::verify_hash(&mut pack, mp, thread_count).unwrap();
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
        mp: Option<&MultiProgress>,
        thread_count: usize,
    ) -> io::Result<()> {
        use std::sync::atomic::{AtomicBool, AtomicUsize};

        let root_name_list = pack.get_root_struct_item_name_list()?;
        let attribute = pack.get_manifest_attribute()?;
        let all_file_count = attribute.file_count();
        let data_len = attribute.data_len();

        // 总进度条 / Main progress bar
        let main_pb = create_pb(mp);
        if let Some(pb) = &main_pb {
            pb.set_length(data_len);
            pb.set_style(
                ProgressStyle::default_bar()
                    .template(PACK_PROGRESS_STYLE_TEMPLATE)
                    .unwrap()
                    .progress_chars("=>-"),
            );
            pb.set_prefix("总进度");
        }

        // 统一工作队列 + 终止控制
        // Unified work queue + termination control
        let queue: Arc<(Mutex<VecDeque<PathBuf>>, Condvar)> =
            Arc::new((Mutex::new(VecDeque::new()), Condvar::new()));
        let pending = Arc::new(AtomicUsize::new(root_name_list.len()));
        let all_done = Arc::new(AtomicBool::new(root_name_list.is_empty()));

        {
            let mut q = queue.0.lock().unwrap();
            for name in &root_name_list {
                q.push_back(PathBuf::from(name));
            }
        }

        // 创建子线程进度条 / Create per-worker progress bars
        let mut worker_pbs: Vec<Option<ProgressBar>> = Vec::with_capacity(thread_count);
        for i in 0..thread_count {
            let pb = create_pb(mp);
            if let Some(pb) = &pb {
                pb.set_style(
                    ProgressStyle::default_bar()
                        .template(PACK_PROGRESS_STYLE_TEMPLATE)
                        .unwrap()
                        .progress_chars("=>-"),
                );
                pb.set_prefix(format!("线程{i} "));
            }
            worker_pbs.push(pb);
        }

        let (tx, rx) = mpsc::channel();
        let mut handles = Vec::with_capacity(thread_count);

        for _ in 0..thread_count {
            Self::wpfp_h_work(
                pack,
                &queue,
                &pending,
                &all_done,
                &tx,
                &mut worker_pbs,
                &mut handles,
            );
        }
        drop(tx);

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
                if let Some(pb) = &main_pb {
                    pb.set_position(verified_len);
                    pb.set_message(format!("[{verified_count}/{all_file_count}个文件]"));
                }
                last_pb_update_len = verified_len;
            }

            if let Some(ok) = result
                && !ok
            {
                filed_count += 1;
                warn!(r#"[{filed_count}]虚拟文件 "{path_str}" 哈希校验未通过"#);
            }
        }

        for h in handles {
            h.join().unwrap()?;
        }

        Ok(())
    }

    fn wpfp_h_work(
        pack: &ManagerSync,
        queue: &Arc<(Mutex<VecDeque<PathBuf>>, Condvar)>,
        pending: &Arc<AtomicUsize>,
        all_done: &Arc<AtomicBool>,
        tx: &Sender<(String, u64, Option<bool>)>,
        worker_pbs: &mut Vec<Option<ProgressBar>>,
        handles: &mut Vec<JoinHandle<Result<(), Error>>>,
    ) {
        use std::thread;
        let mut pack = pack.clone();
        let queue = queue.clone();
        let pending = pending.clone();
        let all_done = all_done.clone();
        let tx = tx.clone();
        let w_pb = worker_pbs.pop().unwrap();

        handles.push(thread::spawn(move || -> io::Result<()> {
            loop {
                // 1. 从队列取路径（空队列时 Condvar 挂起）
                //    Pop path from queue (Condvar suspend when empty)
                let path = {
                    let (lock, cvar) = &*queue;
                    let mut q = lock.lock().unwrap();
                    loop {
                        if let Some(p) = q.pop_front() {
                            break p;
                        }
                        if all_done.load(Ordering::SeqCst) {
                            cvar.notify_all();
                            return Ok(());
                        }
                        if let Some(pb) = &w_pb {
                            pb.set_style(
                                ProgressStyle::default_bar()
                                    .template(SPINNER_TEMPLATE)
                                    .unwrap(),
                            );
                            pb.set_message("已挂起");
                        }
                        q = cvar.wait(q).unwrap();
                        if let Some(pb) = &w_pb {
                            pb.set_style(
                                ProgressStyle::default_bar()
                                    .template(PACK_PROGRESS_STYLE_TEMPLATE)
                                    .unwrap()
                                    .progress_chars("=>-"),
                            );
                        }
                    }
                };

                // 2. 判定路径类型（按需加载目录结构或文件元数据）
                //    Determine path type (lazy-load dir struct or file metadata)
                let item = match pack.get_pack_struct_item(&path) {
                    Ok(v) => v,
                    Err(err) => {
                        error!(r#"无法获取虚拟路径"{}"结构项, err:{err}"#, path.display());
                        if pending.fetch_sub(1, Ordering::SeqCst) == 1 {
                            all_done.store(true, Ordering::SeqCst);
                            queue.1.notify_all();
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
                                if pending.fetch_sub(1, Ordering::SeqCst) == 1 {
                                    all_done.store(true, Ordering::SeqCst);
                                    queue.1.notify_all();
                                }
                                continue;
                            }
                        };

                        let n = children.len();
                        if n > 0 {
                            pending.fetch_add(n, Ordering::SeqCst);
                            let mut q = queue.0.lock().unwrap();
                            for child in children {
                                q.push_back(path.join(child));
                            }
                            // 唤醒所有等待线程 / Wake all waiting threads
                            queue.1.notify_all();
                        }
                        // 目录自身处理完毕 / Directory itself resolved
                        if pending.fetch_sub(1, Ordering::SeqCst) == 1 {
                            all_done.store(true, Ordering::SeqCst);
                            queue.1.notify_all();
                        }
                    }
                    PackStructItemType::File { .. } => {
                        let mut rw = match pack.open_virtual_file(&path) {
                            Ok(v) => v,
                            Err(err) => {
                                error!("无法获取包文件读写器，err: {err}");
                                let _ = tx.send((path.display().to_string(), 0u64, None));
                                if pending.fetch_sub(1, Ordering::SeqCst) == 1 {
                                    all_done.store(true, Ordering::SeqCst);
                                    queue.1.notify_all();
                                }
                                continue;
                            }
                        };

                        let file_len = rw.get_len();

                        let path_str = path.display().to_string();
                        let result = match rw.verify_hash(Some(
                            &(|done, total| {
                                if let Some(pb) = &w_pb {
                                    pb.set_length(total);
                                    pb.set_position(done);
                                    pb.set_message(format!("\tFile path: {path_str}"));
                                }
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
                        if pending.fetch_sub(1, Ordering::SeqCst) == 1 {
                            all_done.store(true, Ordering::SeqCst);
                            queue.1.notify_all();
                        }
                    }
                }
            }
        }));
    }
}
