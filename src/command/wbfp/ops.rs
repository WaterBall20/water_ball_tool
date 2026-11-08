//! wbfp TUI 任务执行器 / wbfp TUI job runners
//!
//! 与经典进度条路径共用 `common` 中的复制/解包核心，但进度通过
//! [`JobEvent`] 发送给 UI，而不是写入 indicatif。复制/移动传输使用建议线程数并行复制文件。
//!
//! Shares the copy/extract core in `common` with the classic progress-bar path,
//! but reports progress through [`JobEvent`] instead of indicatif. Copy/move
//! transfers copy files in parallel with the recommended worker count.

//AI=== TUI 任务执行：pack / unpack / verify / transfer / TUI job runners
use super::common::{self, CopyOutcome};
use super::events::{JobEvent, JobSummary, RowStatus};
use crate::command::BUF_LEN;
use std::collections::VecDeque;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::thread::JoinHandle;
use tracing::{error, info, warn};
use water_ball_tool_lib::concurrent_ring::{ConcurrentRing, ConcurrentRingErrorKind};
use water_ball_tool_lib::file_finder::{FileFinder, FileInfo, FileKind, SearchEvent};
use water_ball_tool_lib::tools::PathTool;
use water_ball_tool_lib::wb_files_pack::manager_sync::ManagerSync;
use water_ball_tool_lib::wb_files_pack::{PackFileMetadataRun, PackStructItem, PackStructItemType};

/// UI 启动的后台任务 / background job started by the UI.
#[derive(Debug, Clone)]
pub(super) enum JobSpec {
    Pack {
        in_path: PathBuf,
        pack_path: PathBuf,
        separate_manifest: bool,
        write_optimization: bool,
        thread_count: usize,
    },
    Unpack {
        // 与浏览共用同一个只读句柄：载入的 .lock 不允许同进程再打开一次
        // Share the read-only handle with the browser: the pack's .lock forbids a second open in the same process.
        pack: ManagerSync,
        out_dir: PathBuf,
        hash_verify: bool,
        thread_count: usize,
    },
    Verify {
        pack: ManagerSync,
        thread_count: usize,
    },
    /// 文件系统/包之间的复制或移动 / copy or move between the filesystem and a pack.
    Transfer {
        source: TransferEndpoint,
        dest: TransferEndpoint,
        move_entry: bool,
        thread_count: usize,
    },
}

/// 传输端点：本地路径或包内虚拟路径 / transfer endpoint: a local path or a pack-relative path.
#[derive(Debug, Clone)]
pub(super) enum TransferEndpoint {
    Local(PathBuf),
    Pack { pack: ManagerSync, path: PathBuf },
}

impl TransferEndpoint {
    fn path(&self) -> &Path {
        match self {
            Self::Local(path) | Self::Pack { path, .. } => path,
        }
    }

    fn kind(&self) -> EndpointKind {
        match self {
            Self::Local(_) => EndpointKind::Local,
            Self::Pack { .. } => EndpointKind::Pack,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EndpointKind {
    Local,
    Pack,
}

/// 启动任务线程；线程结束时发送 `Finished`。
///
/// Start the job thread; it sends `Finished` when it ends.
pub(super) fn spawn(spec: JobSpec, tx: Sender<JobEvent>) -> io::Result<JoinHandle<()>> {
    thread::Builder::new()
        .name("wbfp-tui-job".to_string())
        .spawn(move || {
            let result = match spec {
                JobSpec::Pack {
                    in_path,
                    pack_path,
                    separate_manifest,
                    write_optimization,
                    thread_count,
                } => {
                    info!(
                        "TUI job pack: in={}, out={}, threads={thread_count}, separate_manifest={separate_manifest}, write_optimization={write_optimization}",
                        in_path.display(),
                        pack_path.display()
                    );
                    run_pack(
                        &in_path,
                        &pack_path,
                        separate_manifest,
                        write_optimization,
                        thread_count,
                        &tx,
                    )
                }
                JobSpec::Unpack {
                    pack,
                    out_dir,
                    hash_verify,
                    thread_count,
                } => {
                    info!(
                        "TUI job unpack: out={}, threads={thread_count}, hash_verify={hash_verify}",
                        out_dir.display()
                    );
                    run_unpack(pack, &out_dir, hash_verify, thread_count, &tx)
                }
                JobSpec::Verify { pack, thread_count } => {
                    info!("TUI job verify: threads={thread_count}");
                    run_verify(pack, thread_count, &tx)
                }
                JobSpec::Transfer {
                    source,
                    dest,
                    move_entry,
                    thread_count,
                } => {
                    info!(
                        "TUI job transfer: move={move_entry}, threads={thread_count}, src={}, dest={}",
                        source.path().display(),
                        dest.path().display()
                    );
                    run_transfer(source, dest, move_entry, thread_count, &tx)
                }
            };
            match &result {
                Ok(summary) => info!(
                    "TUI job finished: files_done={}, bytes={}, failed={}, warnings={}",
                    summary.files_done, summary.bytes, summary.failed, summary.warnings
                ),
                Err(e) => error!("TUI job failed: {e}"),
            }
            let _ = tx.send(JobEvent::Finished { result });
        })
}

fn emit_total(tx: &Sender<JobEvent>, done: u64, total: u64, files_done: u64, files_total: u64) {
    let _ = tx.send(JobEvent::Total {
        done,
        total,
        files_done,
        files_total,
    });
}

pub(super) fn run_pack(
    in_path: &Path,
    pack_path: &Path,
    separate_manifest: bool,
    write_optimization: bool,
    thread_count: usize,
    tx: &Sender<JobEvent>,
) -> Result<JobSummary, String> {
    if !in_path.exists() {
        return Err(format!(
            "输入路径不存在 / input path not found: {}",
            in_path.display()
        ));
    }
    let pack = if pack_path.exists() {
        info!("包文件已存在，将打开并修改 / opening the existing pack");
        if write_optimization {
            info!(
                "已启用写入优化，未更改的文件将跳过 / write optimization enabled, unchanged files are skipped"
            );
        }
        ManagerSync::options()
            .read(true)
            .write(true)
            .open(&pack_path)
            .map_err(|e| format!("打开包文件错误 / failed to open pack file: {e}"))?
    } else {
        info!("创建新包文件并初始化 / creating a new pack");
        ManagerSync::options()
            .write(true)
            .create_new(true)
            .separate_manifest(separate_manifest)
            .open(&pack_path)
            .map_err(|e| format!("创建包文件错误 / failed to create pack file: {e}"))?
    };

    emit_total(tx, 0, 0, 0, 0);

    if in_path.is_file() {
        let mut file_name = in_path
            .file_name()
            .and_then(|v| v.to_str())
            .unwrap_or_default()
            .to_string();
        file_name.push_str(".wbfp");
        let metadata = in_path
            .metadata()
            .map_err(|e| format!("读取输入文件信息失败 / failed to stat input file: {e}"))?;
        let file_info = FileInfo::new(
            file_name.clone(),
            metadata.len(),
            FileFinder::get_file_modified(&metadata),
            FileKind::File { hash: None },
        );
        let _ = tx.send(JobEvent::File {
            path: in_path.to_path_buf(),
        });
        let _ = tx.send(JobEvent::Start {
            path: in_path.to_path_buf(),
            total: metadata.len(),
        });
        let mut run_buf = vec![0u8; 1024 * 1024];
        let mut last = 0u64;
        let mut reporter = |done: u64, _add: u64, info: &FileInfo, in_p: &Path, _pack_p: &Path| {
            if done == info.length() || done.saturating_sub(last) >= 4 * 1024 * 1024 {
                let _ = tx.send(JobEvent::Progress {
                    path: in_p.to_path_buf(),
                    done,
                    total: info.length(),
                });
                last = done;
            }
        };
        let mut pack = pack;
        let outcome = common::copy_file_into_pack(
            &mut pack,
            &mut run_buf,
            &file_info,
            in_path,
            Path::new(&file_name),
            None,
            &mut reporter,
            write_optimization,
        );
        let bytes = if outcome.is_success() {
            metadata.len()
        } else {
            0
        };
        let status = match outcome {
            CopyOutcome::Written => RowStatus::Done,
            // 未变化是正常的跳过，不是警告 / unchanged is a normal skip, not a warning
            CopyOutcome::Unchanged => RowStatus::Done,
            CopyOutcome::Failed => RowStatus::Failed,
        };
        let _ = tx.send(JobEvent::Done {
            path: in_path.to_path_buf(),
            status,
        });
        emit_total(
            tx,
            bytes,
            metadata.len(),
            u64::from(outcome.is_success()),
            1,
        );
        return Ok(JobSummary {
            files_done: u64::from(outcome.is_success()),
            bytes,
            failed: u64::from(!outcome.is_success()),
            warnings: 0,
        });
    }

    if !in_path.is_dir() {
        return Err(format!(
            "输入路径既不是文件也不是目录 / input is neither a file nor a directory: {}",
            in_path.display()
        ));
    }

    let thread_count = thread_count.max(1);
    let results = ConcurrentRing::new();
    let ff_end = Arc::new(AtomicBool::new(false));
    let file_count = Arc::new(AtomicU64::new(0));
    let data_len = Arc::new(AtomicU64::new(0));
    let done_bytes = Arc::new(AtomicU64::new(0));
    let done_files = Arc::new(AtomicU64::new(0));
    let search = spawn_search(
        in_path,
        results.clone(),
        &ff_end,
        &file_count,
        &data_len,
        &done_bytes,
        &done_files,
        tx.clone(),
    )
    .map_err(|e| format!("创建搜索线程失败 / failed to spawn search thread: {e}"))?;

    let (agg_tx, agg_rx) = mpsc::channel::<(u64, bool)>();
    let workers = match spawn_copy_workers(
        &pack,
        results,
        in_path,
        &ff_end,
        thread_count,
        write_optimization,
        tx.clone(),
        &agg_tx,
        &done_bytes,
        &done_files,
    ) {
        Ok(workers) => workers,
        Err(e) => {
            ff_end.store(true, Ordering::Release);
            let _ = search.join();
            return Err(e);
        }
    };
    drop(agg_tx);

    let mut failed = 0u64;
    let mut files_done = 0u64;
    let mut written = 0u64;
    let mut last_log = 0u64;
    for (bytes, is_failed) in agg_rx {
        written += bytes;
        files_done += 1;
        if is_failed {
            failed += 1;
        }
        let total = data_len.load(Ordering::Acquire).max(written);
        emit_total(
            tx,
            written,
            total,
            files_done,
            file_count.load(Ordering::Acquire),
        );
        if written.saturating_sub(last_log) >= 1 << 30 {
            info!("pack progress: {written}/{total} bytes, {files_done} files, failed={failed}");
            last_log = written;
        }
    }

    if search
        .join()
        .map_err(|_| "文件搜索线程异常退出 / search thread panicked".to_string())?
        .is_err()
    {
        return Err("文件搜索失败 / file search failed".to_string());
    }
    ff_end.store(true, Ordering::Release);
    for worker in workers {
        worker
            .join()
            .map_err(|_| "复制线程异常退出 / copy thread panicked".to_string())?
            .map_err(|e| format!("复制线程失败 / copy worker failed: {e}"))?;
    }

    let total = data_len.load(Ordering::Acquire);
    emit_total(
        tx,
        written,
        total,
        files_done,
        file_count.load(Ordering::Acquire),
    );
    Ok(JobSummary {
        files_done,
        bytes: written,
        failed,
        warnings: 0,
    })
}

fn spawn_search(
    in_path: &Path,
    results: ConcurrentRing<(PathBuf, FileInfo)>,
    ff_end: &Arc<AtomicBool>,
    file_count: &Arc<AtomicU64>,
    data_len: &Arc<AtomicU64>,
    done_bytes: &Arc<AtomicU64>,
    done_files: &Arc<AtomicU64>,
    tx: Sender<JobEvent>,
) -> io::Result<JoinHandle<io::Result<()>>> {
    let in_dir_path = in_path.to_path_buf();
    let ff_end = ff_end.clone();
    let file_count = file_count.clone();
    let data_len = data_len.clone();
    let done_bytes = done_bytes.clone();
    let done_files = done_files.clone();
    let worker = move || -> io::Result<()> {
        let result = (|| -> io::Result<()> {
            let ff = FileFinder;
            let dir_count = Arc::new(AtomicU64::new(0));
            let (progress_tx, progress_rx) = mpsc::channel();
            let (search_handle, stream_results) =
                ff.search_stream(&in_dir_path, true, progress_tx, 2)?;

            let progress_worker = {
                let file_count = file_count.clone();
                let dir_count = dir_count.clone();
                let data_len = data_len.clone();
                let done_bytes = done_bytes.clone();
                let done_files = done_files.clone();
                let tx = tx.clone();
                move || {
                    for (add_file, add_dir) in progress_rx {
                        file_count.fetch_add(add_file, Ordering::Release);
                        dir_count.fetch_add(add_dir, Ordering::Release);
                        let all_count =
                            file_count.load(Ordering::Acquire) + dir_count.load(Ordering::Acquire);
                        if all_count.is_multiple_of(100) {
                            emit_total(
                                &tx,
                                done_bytes.load(Ordering::Acquire),
                                data_len.load(Ordering::Acquire),
                                done_files.load(Ordering::Acquire),
                                file_count.load(Ordering::Acquire),
                            );
                        }
                    }
                }
            };
            let progress_handle = thread::Builder::new()
                .name("wbfp-tui-search-progress".to_string())
                .spawn(progress_worker)?;

            for event in stream_results {
                match event {
                    SearchEvent::Entry(path, info) => match info.file_kind() {
                        FileKind::File { .. } => {
                            data_len.fetch_add(info.length(), Ordering::Release);
                            let _ = tx.send(JobEvent::File { path: path.clone() });
                            let _ = results.push((path, info));
                        }
                        // 打包不复制目录；空目录没有文件事件，因此也不发 Dir 事件，否则会一直显示进行中。
                        // Pack does not copy dirs; an empty dir has no file event, so no Dir event (it would spin forever).
                        FileKind::Dir { .. } => {}
                    },
                    SearchEvent::Warning(warning) => {
                        warn!(
                            "文件搜索警告 [{:?}]: {:?}",
                            warning.warning_type, warning.path
                        );
                    }
                }
            }

            search_handle.join().map_err(|_| {
                io::Error::other("文件搜索子线程异常退出 / search stream thread panicked")
            })??;
            progress_handle.join().map_err(|_| {
                io::Error::other("搜索进度线程异常退出 / search progress thread panicked")
            })?;
            emit_total(
                &tx,
                done_bytes.load(Ordering::Acquire),
                data_len.load(Ordering::Acquire),
                done_files.load(Ordering::Acquire),
                file_count.load(Ordering::Acquire),
            );
            Ok(())
        })();
        ff_end.store(true, Ordering::Release);
        result
    };
    thread::Builder::new()
        .name("wbfp-tui-search".to_string())
        .spawn(worker)
}

fn spawn_copy_workers(
    pack: &ManagerSync,
    results: ConcurrentRing<(PathBuf, FileInfo)>,
    in_path: &Path,
    ff_end: &Arc<AtomicBool>,
    thread_count: usize,
    write_optimization: bool,
    tx: Sender<JobEvent>,
    agg_tx: &Sender<(u64, bool)>,
    done_bytes: &Arc<AtomicU64>,
    done_files: &Arc<AtomicU64>,
) -> Result<Vec<JoinHandle<io::Result<()>>>, String> {
    let mut handles = Vec::with_capacity(thread_count);
    for index in 0..thread_count {
        let pack = pack.clone();
        let results = results.clone();
        let in_dir_path = in_path.to_path_buf();
        let ff_end = ff_end.clone();
        let stop_end = ff_end.clone();
        let tx = tx.clone();
        let agg_tx = agg_tx.clone();
        let done_bytes = done_bytes.clone();
        let done_files = done_files.clone();
        let worker = move || -> io::Result<()> {
            run_copy_worker(
                pack,
                results,
                &in_dir_path,
                &ff_end,
                write_optimization,
                &tx,
                &agg_tx,
                &done_bytes,
                &done_files,
            );
            Ok(())
        };
        match thread::Builder::new()
            .name(format!("wbfp-tui-pack-worker-{index}"))
            .spawn(worker)
        {
            Ok(handle) => handles.push(handle),
            Err(e) => {
                stop_end.store(true, Ordering::Release);
                for handle in handles {
                    if handle.join().is_err() {
                        error!("打包工作线程异常退出 / pack worker panicked");
                    }
                }
                return Err(format!(
                    "创建打包线程失败 / failed to spawn pack worker: {e}"
                ));
            }
        }
    }
    Ok(handles)
}

fn run_copy_worker(
    mut pack: ManagerSync,
    results: ConcurrentRing<(PathBuf, FileInfo)>,
    in_dir_path: &Path,
    ff_end: &AtomicBool,
    write_optimization: bool,
    tx: &Sender<JobEvent>,
    agg_tx: &Sender<(u64, bool)>,
    done_bytes: &AtomicU64,
    done_files: &AtomicU64,
) {
    let mut run_buf = vec![0u8; 1024 * 1024];
    loop {
        let Some((file_path, file_info)) = pop_file(&results, ff_end) else {
            return;
        };
        let Some(pack_path) = PathTool::path_remove_head(file_path.as_path(), in_dir_path) else {
            error!(
                r#"无法从路径"{}"中去除输入目录前缀"{}"，将跳过该文件"#,
                file_path.display(),
                in_dir_path.display()
            );
            let _ = tx.send(JobEvent::Done {
                path: file_path,
                status: RowStatus::Failed,
            });
            let _ = agg_tx.send((0, true));
            continue;
        };
        let _ = tx.send(JobEvent::Start {
            path: file_path.clone(),
            total: file_info.length(),
        });
        let mut last = 0u64;
        let mut reporter = |done: u64, _add: u64, info: &FileInfo, in_p: &Path, _pack_p: &Path| {
            if done == info.length() || done.saturating_sub(last) >= 4 * 1024 * 1024 {
                let _ = tx.send(JobEvent::Progress {
                    path: in_p.to_path_buf(),
                    done,
                    total: info.length(),
                });
                last = done;
            }
        };
        let outcome = common::copy_file_into_pack(
            &mut pack,
            &mut run_buf,
            &file_info,
            &file_path,
            &pack_path,
            None,
            &mut reporter,
            write_optimization,
        );
        let status = match outcome {
            CopyOutcome::Written => RowStatus::Done,
            // 未变化是正常的跳过，不是警告 / unchanged is a normal skip, not a warning
            CopyOutcome::Unchanged => RowStatus::Done,
            CopyOutcome::Failed => RowStatus::Failed,
        };
        let bytes = if outcome.is_success() {
            file_info.length()
        } else {
            0
        };
        let _ = tx.send(JobEvent::Done {
            path: file_path,
            status,
        });
        let _ = agg_tx.send((bytes, outcome == CopyOutcome::Failed));
        done_bytes.fetch_add(bytes, Ordering::Release);
        done_files.fetch_add(1, Ordering::Release);
    }
}

fn pop_file(
    results: &ConcurrentRing<(PathBuf, FileInfo)>,
    ff_end: &AtomicBool,
) -> Option<(PathBuf, FileInfo)> {
    if let Some(v) = results.pop() {
        return Some(v);
    }
    if ff_end.load(Ordering::Acquire) {
        return None;
    }
    match results.pop_wait() {
        Ok(v) => Some(v),
        Err(e) => {
            if !matches!(
                e.kind,
                ConcurrentRingErrorKind::WritersClosed
                    | ConcurrentRingErrorKind::AllWaiting
                    | ConcurrentRingErrorKind::SingleInstance
            ) {
                error!("队列等待文件信息时发生错误, err: {e}");
            }
            None
        }
    }
}

fn run_unpack(
    mut pack: ManagerSync,
    out_dir: &Path,
    hash_verify: bool,
    thread_count: usize,
    tx: &Sender<JobEvent>,
) -> Result<JobSummary, String> {
    fs::create_dir_all(out_dir)
        .map_err(|e| format!("创建输出目录失败 / failed to create output dir: {e}"))?;
    let root = pack
        .get_root_struct_item_name_list()
        .map_err(|e| format!("读取包根目录失败 / failed to read pack root: {e}"))?;
    let attribute = pack
        .get_manifest_attribute()
        .map_err(|e| format!("读取包属性失败 / failed to read pack attribute: {e}"))?;
    let files_total = attribute.file_count();
    let total = attribute.data_len();
    info!("unpack start: files_total={files_total}, data_len={total}");
    emit_total(tx, 0, total, 0, files_total);

    if root.is_empty() {
        return Ok(JobSummary::default());
    }

    let thread_count = thread_count.max(1);
    let queue = ConcurrentRing::new();
    for name in &root {
        let _ = queue.push(PathBuf::from(name));
    }
    let pending = Arc::new(AtomicUsize::new(root.len()));
    let all_done = Arc::new(AtomicBool::new(false));
    let (agg_tx, agg_rx) = mpsc::channel::<(u64, bool, bool)>();
    let mut handles = Vec::with_capacity(thread_count);
    for index in 0..thread_count {
        let pack = pack.clone();
        let queue = queue.clone();
        let pending = pending.clone();
        let all_done = all_done.clone();
        let stop_done = all_done.clone();
        let tx = tx.clone();
        let agg_tx = agg_tx.clone();
        let out_dir = out_dir.to_path_buf();
        let worker = move || -> io::Result<()> {
            run_unpack_worker(
                pack,
                queue,
                pending,
                all_done,
                tx,
                agg_tx,
                out_dir,
                hash_verify,
            );
            Ok(())
        };
        match thread::Builder::new()
            .name(format!("wbfp-tui-unpack-worker-{index}"))
            .spawn(worker)
        {
            Ok(handle) => handles.push(handle),
            Err(e) => {
                stop_done.store(true, Ordering::Release);
                for handle in handles {
                    if handle.join().is_err() {
                        error!("解包工作线程异常退出 / unpack worker panicked");
                    }
                }
                return Err(format!(
                    "创建解包线程失败 / failed to spawn unpack worker: {e}"
                ));
            }
        }
    }
    // 释放主线程持有的队列实例，等待线程才能触发 AllWaiting 退出 / drop the main queue clone so waiters can observe AllWaiting
    drop(queue);
    drop(agg_tx);

    let mut files_done = 0u64;
    let mut written = 0u64;
    let mut failed = 0u64;
    let mut warnings = 0u64;
    let mut last_log = 0u64;
    for (bytes, is_failed, is_warning) in agg_rx {
        written += bytes;
        files_done += 1;
        if is_failed {
            failed += 1;
        }
        if is_warning {
            warnings += 1;
        }
        emit_total(tx, written, total, files_done, files_total);
        if written.saturating_sub(last_log) >= 1 << 30 {
            info!(
                "progress: {written}/{total} bytes, {files_done}/{files_total} files, failed={failed}, warnings={warnings}"
            );
            last_log = written;
        }
    }

    for handle in handles {
        handle
            .join()
            .map_err(|_| "解包线程异常退出 / unpack thread panicked".to_string())?
            .map_err(|e| format!("解包线程失败 / unpack worker failed: {e}"))?;
    }
    emit_total(tx, written, total, files_done, files_total);
    Ok(JobSummary {
        files_done,
        bytes: written,
        failed,
        warnings,
    })
}

fn run_unpack_worker(
    mut pack: ManagerSync,
    queue: ConcurrentRing<PathBuf>,
    pending: Arc<AtomicUsize>,
    all_done: Arc<AtomicBool>,
    tx: Sender<JobEvent>,
    agg_tx: Sender<(u64, bool, bool)>,
    out_dir: PathBuf,
    hash_verify: bool,
) {
    let mut run_buf = vec![0u8; BUF_LEN];
    loop {
        let Some(path) = queue_path(&queue, &all_done) else {
            return;
        };
        let item = match pack.get_pack_struct_item(&path) {
            Ok(item) => item,
            Err(err) => {
                error!(r#"无法获取虚拟路径"{}"结构项, err:{err}"#, path.display());
                let _ = tx.send(JobEvent::Done {
                    path,
                    status: RowStatus::Failed,
                });
                finish_item(&pending, &all_done);
                continue;
            }
        };
        match item.item_type() {
            PackStructItemType::Dir { .. } => {
                let _ = tx.send(JobEvent::Dir { path: path.clone() });
                let out = out_dir.join(&path);
                let dir_status = match fs::create_dir_all(&out) {
                    Ok(()) => None,
                    Err(err) => {
                        error!(r#"无法创建目录"{}", err:{err}"#, out.display());
                        Some(RowStatus::Failed)
                    }
                };
                let (children, children_ok) = match pack.get_struct_item_name_list(&path) {
                    Ok(children) => (children, true),
                    Err(err) => {
                        error!(
                            r#"无法获取虚拟路径"{}"的子项名称, err:{err}"#,
                            path.display()
                        );
                        (Vec::new(), false)
                    }
                };
                let own_status = if children_ok {
                    dir_status
                } else {
                    Some(RowStatus::Failed)
                };
                if let Some(status) = own_status {
                    let _ = tx.send(JobEvent::Done {
                        path: path.clone(),
                        status,
                    });
                }
                if children_ok {
                    let n = children.len();
                    if n > 0 {
                        pending.fetch_add(n, Ordering::Release);
                        for child in children {
                            let _ = queue.push(path.join(child));
                        }
                    } else if own_status.is_none() {
                        // 空目录没有子项事件，直接完成 / an empty directory has no child events; finish it now
                        let _ = tx.send(JobEvent::Done {
                            path: path.clone(),
                            status: RowStatus::Done,
                        });
                    }
                }
                finish_item(&pending, &all_done);
            }
            PackStructItemType::File { .. } => {
                let out_path = out_dir.join(&path);
                let file_total = metadata_len(&item);
                let _ = tx.send(JobEvent::Start {
                    path: path.clone(),
                    total: file_total,
                });
                let hash_ok = if hash_verify {
                    let mut rw = match pack.open_virtual_file(&path) {
                        Ok(rw) => rw,
                        Err(err) => {
                            error!("无法打开虚拟文件\"{}\", err: {err:?}", path.display());
                            let _ = tx.send(JobEvent::Done {
                                path,
                                status: RowStatus::Failed,
                            });
                            let _ = agg_tx.send((0, true, false));
                            finish_item(&pending, &all_done);
                            continue;
                        }
                    };
                    let progress = |done: u64, total: u64| {
                        let _ = tx.send(JobEvent::Progress {
                            path: path.clone(),
                            done,
                            total,
                        });
                    };
                    match rw.verify_hash(Some(&progress)) {
                        Ok(ok) => ok,
                        Err(err) => {
                            warn!(
                                "虚拟文件\"{}\"哈希验证发生错误, err: {err:?}, 将跳过解包",
                                path.display()
                            );
                            false
                        }
                    }
                } else {
                    true
                };

                if !hash_ok {
                    warn!("虚拟文件\"{}\"哈希校验未通过，将跳过解包", path.display());
                    let _ = tx.send(JobEvent::Done {
                        path,
                        status: RowStatus::Warning,
                    });
                    let _ = agg_tx.send((0, false, true));
                    finish_item(&pending, &all_done);
                    continue;
                }

                let progress = |done: u64, total: u64| {
                    let _ = tx.send(JobEvent::Progress {
                        path: path.clone(),
                        done,
                        total,
                    });
                };
                let result = common::extract_pack_file_to_disk(
                    &mut pack,
                    &path,
                    &out_path,
                    &mut run_buf,
                    Some(&progress),
                );
                match result {
                    Ok(bytes) => {
                        let _ = tx.send(JobEvent::Done {
                            path,
                            status: RowStatus::Done,
                        });
                        let _ = agg_tx.send((bytes, false, false));
                    }
                    Err(err) => {
                        error!("解包文件\"{}\"失败, err:{err}", path.display());
                        let _ = tx.send(JobEvent::Done {
                            path,
                            status: RowStatus::Failed,
                        });
                        let _ = agg_tx.send((0, true, false));
                    }
                }
                finish_item(&pending, &all_done);
            }
        }
    }
}

fn run_verify(
    mut pack: ManagerSync,
    thread_count: usize,
    tx: &Sender<JobEvent>,
) -> Result<JobSummary, String> {
    let root = pack
        .get_root_struct_item_name_list()
        .map_err(|e| format!("读取包根目录失败 / failed to read pack root: {e}"))?;
    let attribute = pack
        .get_manifest_attribute()
        .map_err(|e| format!("读取包属性失败 / failed to read pack attribute: {e}"))?;
    let files_total = attribute.file_count();
    let total = attribute.data_len();
    info!("verify start: files_total={files_total}, data_len={total}");
    emit_total(tx, 0, total, 0, files_total);

    if root.is_empty() {
        return Ok(JobSummary::default());
    }

    let thread_count = thread_count.max(1);
    let queue = ConcurrentRing::new();
    for name in &root {
        let _ = queue.push(PathBuf::from(name));
    }
    let pending = Arc::new(AtomicUsize::new(root.len()));
    let all_done = Arc::new(AtomicBool::new(false));
    let (agg_tx, agg_rx) = mpsc::channel::<(u64, bool, bool)>();
    let mut handles = Vec::with_capacity(thread_count);
    for index in 0..thread_count {
        let pack = pack.clone();
        let queue = queue.clone();
        let pending = pending.clone();
        let all_done = all_done.clone();
        let stop_done = all_done.clone();
        let tx = tx.clone();
        let agg_tx = agg_tx.clone();
        let worker = move || -> io::Result<()> {
            run_verify_worker(pack, queue, pending, all_done, tx, agg_tx);
            Ok(())
        };
        match thread::Builder::new()
            .name(format!("wbfp-tui-verify-worker-{index}"))
            .spawn(worker)
        {
            Ok(handle) => handles.push(handle),
            Err(e) => {
                stop_done.store(true, Ordering::Release);
                for handle in handles {
                    if handle.join().is_err() {
                        error!("哈希校验工作线程异常退出 / verify worker panicked");
                    }
                }
                return Err(format!(
                    "创建哈希校验线程失败 / failed to spawn verify worker: {e}"
                ));
            }
        }
    }
    // 释放主线程持有的队列实例，等待线程才能触发 AllWaiting 退出 / drop the main queue clone so waiters can observe AllWaiting
    drop(queue);
    drop(agg_tx);

    let mut files_done = 0u64;
    let mut written = 0u64;
    let mut failed = 0u64;
    let mut warnings = 0u64;
    let mut last_log = 0u64;
    for (bytes, is_failed, is_warning) in agg_rx {
        written += bytes;
        files_done += 1;
        if is_failed {
            failed += 1;
        }
        if is_warning {
            warnings += 1;
        }
        emit_total(tx, written, total, files_done, files_total);
        if written.saturating_sub(last_log) >= 1 << 30 {
            info!(
                "progress: {written}/{total} bytes, {files_done}/{files_total} files, failed={failed}, warnings={warnings}"
            );
            last_log = written;
        }
    }

    for handle in handles {
        handle
            .join()
            .map_err(|_| "哈希校验线程异常退出 / verify thread panicked".to_string())?
            .map_err(|e| format!("哈希校验线程失败 / verify worker failed: {e}"))?;
    }
    emit_total(tx, written, total, files_done, files_total);
    Ok(JobSummary {
        files_done,
        bytes: written,
        failed,
        warnings,
    })
}

fn run_verify_worker(
    mut pack: ManagerSync,
    queue: ConcurrentRing<PathBuf>,
    pending: Arc<AtomicUsize>,
    all_done: Arc<AtomicBool>,
    tx: Sender<JobEvent>,
    agg_tx: Sender<(u64, bool, bool)>,
) {
    loop {
        let Some(path) = queue_path(&queue, &all_done) else {
            return;
        };
        let item = match pack.get_pack_struct_item(&path) {
            Ok(item) => item,
            Err(err) => {
                error!(r#"无法获取虚拟路径"{}"结构项, err:{err}"#, path.display());
                let _ = tx.send(JobEvent::Done {
                    path,
                    status: RowStatus::Failed,
                });
                finish_item(&pending, &all_done);
                continue;
            }
        };
        match item.item_type() {
            PackStructItemType::Dir { .. } => {
                let _ = tx.send(JobEvent::Dir { path: path.clone() });
                match pack.get_struct_item_name_list(&path) {
                    Ok(children) => {
                        let n = children.len();
                        if n > 0 {
                            pending.fetch_add(n, Ordering::Release);
                            for child in children {
                                let _ = queue.push(path.join(child));
                            }
                        } else {
                            // 空目录没有子项事件，直接完成 / an empty directory has no child events
                            let _ = tx.send(JobEvent::Done {
                                path,
                                status: RowStatus::Done,
                            });
                        }
                    }
                    Err(err) => {
                        error!(
                            r#"无法获取虚拟路径"{}"的子项名称, err:{err}"#,
                            path.display()
                        );
                        let _ = tx.send(JobEvent::Done {
                            path,
                            status: RowStatus::Failed,
                        });
                    }
                }
                finish_item(&pending, &all_done);
            }
            PackStructItemType::File { .. } => {
                let mut rw = match pack.open_virtual_file(&path) {
                    Ok(rw) => rw,
                    Err(err) => {
                        error!("无法获取包文件读写器，err: {err}");
                        let _ = tx.send(JobEvent::Done {
                            path,
                            status: RowStatus::Failed,
                        });
                        let _ = agg_tx.send((0, true, false));
                        finish_item(&pending, &all_done);
                        continue;
                    }
                };
                let len = rw.get_len();
                let _ = tx.send(JobEvent::Start {
                    path: path.clone(),
                    total: len,
                });
                let progress = |done: u64, total: u64| {
                    let _ = tx.send(JobEvent::Progress {
                        path: path.clone(),
                        done,
                        total,
                    });
                };
                let (status, is_failed, is_warning) = match rw.verify_hash(Some(&progress)) {
                    Ok(true) => (RowStatus::Done, false, false),
                    Ok(false) => {
                        warn!("虚拟文件 \"{}\" 哈希校验未通过", path.display());
                        (RowStatus::Failed, true, false)
                    }
                    Err(err) => {
                        warn!(
                            r#"虚拟文件"{}"哈希验证发生错误, err: {err:?}"#,
                            path.display()
                        );
                        (RowStatus::Warning, false, true)
                    }
                };
                let _ = tx.send(JobEvent::Done { path, status });
                let _ = agg_tx.send((len, is_failed, is_warning));
                finish_item(&pending, &all_done);
            }
        }
    }
}

fn queue_path(queue: &ConcurrentRing<PathBuf>, all_done: &AtomicBool) -> Option<PathBuf> {
    if let Some(path) = queue.pop() {
        return Some(path);
    }
    if all_done.load(Ordering::Acquire) {
        return None;
    }
    match queue.pop_wait() {
        Ok(path) => Some(path),
        Err(e) => {
            if !matches!(
                e.kind,
                ConcurrentRingErrorKind::WritersClosed
                    | ConcurrentRingErrorKind::AllWaiting
                    | ConcurrentRingErrorKind::SingleInstance
            ) {
                error!("等待路径队列时发生错误, err: {e}");
            }
            None
        }
    }
}

fn finish_item(pending: &AtomicUsize, all_done: &AtomicBool) {
    if pending.fetch_sub(1, Ordering::Release) == 1 {
        all_done.store(true, Ordering::Release);
    }
}

//AI_END===

//AI=== 后台复制/移动任务 / background copy and move jobs
/// 一个待复制的文件 / one file to copy.
struct FileTask {
    src: PathBuf,
    dst: PathBuf,
    size: u64,
}

/// 进度上报状态（每个工作线程各一份）/ progress reporting state, one per worker.
struct ProgressCtx {
    tx: Sender<JobEvent>,
    done_bytes: Arc<AtomicU64>,
    done_files: Arc<AtomicU64>,
    total_bytes: u64,
    files_total: u64,
    path: PathBuf,
    file_total: u64,
    last: u64,
    added: u64,
}

impl ProgressCtx {
    fn add(&mut self, delta: u64, file_done: u64) {
        self.added = self.added.saturating_add(delta);
        let total_done = self
            .done_bytes
            .fetch_add(delta, Ordering::AcqRel)
            .saturating_add(delta);
        if file_done == self.file_total || file_done.saturating_sub(self.last) >= 4 * 1024 * 1024 {
            let _ = self.tx.send(JobEvent::Progress {
                path: self.path.clone(),
                done: file_done,
                total: self.file_total,
            });
            emit_total(
                &self.tx,
                total_done,
                self.total_bytes,
                self.done_files.load(Ordering::Acquire),
                self.files_total,
            );
            self.last = file_done;
        }
    }
}

/// 复制/移动任务主体 / copy or move job body.
fn run_transfer(
    source: TransferEndpoint,
    dest: TransferEndpoint,
    move_entry: bool,
    thread_count: usize,
    tx: &Sender<JobEvent>,
) -> Result<JobSummary, String> {
    let is_dir = endpoint_is_dir(&source)?;

    // 同卷本地移动与包内移动只改元数据，直接完成 / same-volume local and in-pack moves only touch metadata.
    if move_entry {
        match (&source, &dest) {
            (TransferEndpoint::Local(src), TransferEndpoint::Local(dst)) => {
                match fs::rename(src, dst) {
                    Ok(()) => {
                        emit_finished_item(tx, src);
                        return Ok(JobSummary {
                            files_done: 1,
                            ..JobSummary::default()
                        });
                    }
                    Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {}
                    Err(e) => {
                        return Err(format!(
                            "移动失败 / rename failed {} -> {}: {e}",
                            src.display(),
                            dst.display()
                        ));
                    }
                }
            }
            (
                TransferEndpoint::Pack { pack, path: src },
                TransferEndpoint::Pack { path: dst, .. },
            ) => {
                let mut pack = pack.clone();
                pack.move_entry(src, dst)
                    .map_err(|e| format!("包内移动失败 / failed to move inside pack: {e}"))?;
                emit_finished_item(tx, src);
                return Ok(JobSummary {
                    files_done: 1,
                    ..JobSummary::default()
                });
            }
            _ => {}
        }
    }

    run_copy_transfer(source, dest, is_dir, move_entry, thread_count, tx)
}

fn emit_finished_item(tx: &Sender<JobEvent>, path: &Path) {
    let _ = tx.send(JobEvent::Start {
        path: path.to_path_buf(),
        total: 0,
    });
    let _ = tx.send(JobEvent::Done {
        path: path.to_path_buf(),
        status: RowStatus::Done,
    });
    emit_total(tx, 1, 1, 1, 1);
}

fn run_copy_transfer(
    source: TransferEndpoint,
    dest: TransferEndpoint,
    is_dir: bool,
    move_entry: bool,
    thread_count: usize,
    tx: &Sender<JobEvent>,
) -> Result<JobSummary, String> {
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    let mut warnings = 0u64;

    if is_dir {
        create_dest_dir(&dest, dest.path())?;
        let _ = tx.send(JobEvent::Dir {
            path: source.path().to_path_buf(),
        });
        discover(
            &source,
            &dest,
            Path::new(""),
            &mut files,
            &mut dirs,
            tx,
            &mut warnings,
        )?;
        for rel in &dirs {
            create_dest_dir(&dest, &dest.path().join(rel))?;
        }
    } else {
        let size = endpoint_file_len(&source)?;
        let _ = tx.send(JobEvent::File {
            path: source.path().to_path_buf(),
        });
        files.push(FileTask {
            src: source.path().to_path_buf(),
            dst: dest.path().to_path_buf(),
            size,
        });
    }

    let file_count = files.len() as u64;
    let total_bytes: u64 = files.iter().map(|file| file.size).sum();
    emit_total(tx, 0, total_bytes, 0, file_count);

    let queue = Arc::new(Mutex::new(VecDeque::from(files)));
    let done_bytes = Arc::new(AtomicU64::new(0));
    let done_files = Arc::new(AtomicU64::new(0));
    let failed = Arc::new(AtomicU64::new(0));
    let source_pack = match &source {
        TransferEndpoint::Pack { pack, .. } => Some(pack.clone()),
        TransferEndpoint::Local(_) => None,
    };
    let dest_pack = match &dest {
        TransferEndpoint::Pack { pack, .. } => Some(pack.clone()),
        TransferEndpoint::Local(_) => None,
    };
    let workers = thread_count
        .max(1)
        .min(usize::try_from(file_count).unwrap_or(usize::MAX).max(1));
    let mut handles = Vec::with_capacity(workers);
    for index in 0..workers {
        let ctx = TransferWorker {
            queue: queue.clone(),
            tx: tx.clone(),
            done_bytes: done_bytes.clone(),
            done_files: done_files.clone(),
            failed: failed.clone(),
            total_bytes,
            files_total: file_count,
            source_kind: source.kind(),
            dest_kind: dest.kind(),
            source_pack: source_pack.clone(),
            dest_pack: dest_pack.clone(),
        };
        let handle = thread::Builder::new()
            .name(format!("wbfp-transfer-{index}"))
            .spawn(move || transfer_worker(ctx))
            .map_err(|e| format!("创建传输线程失败 / failed to spawn transfer worker: {e}"))?;
        handles.push(handle);
    }
    for handle in handles {
        handle
            .join()
            .map_err(|_| "传输线程异常退出 / transfer worker panicked".to_string())?;
    }

    let files_done = done_files.load(Ordering::Acquire);
    let bytes = done_bytes.load(Ordering::Acquire);
    let failed_count = failed.load(Ordering::Acquire);
    let summary = JobSummary {
        files_done,
        bytes,
        failed: failed_count,
        warnings,
    };

    if move_entry {
        if failed_count == 0 {
            delete_source(&source)?;
        } else {
            // 部分复制的文件保留，便于排查；不做事务回滚。
            // Partially copied files are kept for inspection; there is no rollback.
            return Err(format!(
                "{failed_count} 个文件复制失败，源已保留 / {failed_count} file(s) failed; source kept"
            ));
        }
    }
    Ok(summary)
}

/// 传输工作线程的状态 / per-worker transfer state.
struct TransferWorker {
    queue: Arc<Mutex<VecDeque<FileTask>>>,
    tx: Sender<JobEvent>,
    done_bytes: Arc<AtomicU64>,
    done_files: Arc<AtomicU64>,
    failed: Arc<AtomicU64>,
    total_bytes: u64,
    files_total: u64,
    source_kind: EndpointKind,
    dest_kind: EndpointKind,
    source_pack: Option<ManagerSync>,
    dest_pack: Option<ManagerSync>,
}

fn transfer_worker(mut ctx: TransferWorker) {
    loop {
        let task = ctx
            .queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front();
        let Some(task) = task else {
            return;
        };
        let _ = ctx.tx.send(JobEvent::Start {
            path: task.src.clone(),
            total: task.size,
        });
        let mut progress = ProgressCtx {
            tx: ctx.tx.clone(),
            done_bytes: ctx.done_bytes.clone(),
            done_files: ctx.done_files.clone(),
            total_bytes: ctx.total_bytes,
            files_total: ctx.files_total,
            path: task.src.clone(),
            file_total: task.size,
            last: 0,
            added: 0,
        };
        let result = copy_task(
            &task,
            ctx.source_kind,
            ctx.dest_kind,
            &mut ctx.source_pack,
            &mut ctx.dest_pack,
            &mut progress,
        );
        let status = match result {
            Ok(()) => {
                ctx.done_files.fetch_add(1, Ordering::AcqRel);
                RowStatus::Done
            }
            Err(e) => {
                warn!(
                    "传输文件失败 / transfer failed for {}: {e}",
                    task.src.display()
                );
                ctx.done_bytes.fetch_sub(progress.added, Ordering::AcqRel);
                ctx.failed.fetch_add(1, Ordering::AcqRel);
                RowStatus::Failed
            }
        };
        let _ = ctx.tx.send(JobEvent::Done {
            path: task.src.clone(),
            status,
        });
        emit_total(
            &ctx.tx,
            ctx.done_bytes.load(Ordering::Acquire),
            ctx.total_bytes,
            ctx.done_files.load(Ordering::Acquire),
            ctx.files_total,
        );
    }
}

fn copy_task(
    task: &FileTask,
    source_kind: EndpointKind,
    dest_kind: EndpointKind,
    source_pack: &mut Option<ManagerSync>,
    dest_pack: &mut Option<ManagerSync>,
    progress: &mut ProgressCtx,
) -> Result<(), String> {
    match (source_kind, dest_kind) {
        (EndpointKind::Local, EndpointKind::Local) => copy_local_file(task, progress),
        (EndpointKind::Local, EndpointKind::Pack) => copy_into_pack(task, dest_pack, progress),
        (EndpointKind::Pack, EndpointKind::Local) => copy_out_of_pack(task, source_pack, progress),
        (EndpointKind::Pack, EndpointKind::Pack) => {
            copy_pack_file(task, source_pack, dest_pack, progress)
        }
    }
}

fn copy_local_file(task: &FileTask, progress: &mut ProgressCtx) -> Result<(), String> {
    let mut input = fs::File::open(&task.src).map_err(|e| {
        format!(
            "打开源文件失败 / failed to open source {}: {e}",
            task.src.display()
        )
    })?;
    let metadata = input.metadata().map_err(|e| {
        format!(
            "读取源文件信息失败 / failed to stat source {}: {e}",
            task.src.display()
        )
    })?;
    progress.file_total = metadata.len();
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&task.dst)
        .map_err(|e| {
            format!(
                "创建目标文件失败 / failed to create target {}: {e}",
                task.dst.display()
            )
        })?;
    let written = pump(&mut input, &mut output, progress)?;
    // 源文件可能在扫描后增长；短读才是错误，增长照常完成。
    // The source may grow after discovery; only a short read is an error.
    if written < progress.file_total {
        return Err(format!(
            "文件长度不一致 / size mismatch: {} < {}",
            written, progress.file_total
        ));
    }
    if let Ok(modified) = metadata.modified() {
        let _ = output.set_modified(modified);
    }
    let _ = fs::set_permissions(&task.dst, metadata.permissions());
    Ok(())
}

fn copy_into_pack(
    task: &FileTask,
    dest_pack: &mut Option<ManagerSync>,
    progress: &mut ProgressCtx,
) -> Result<(), String> {
    let pack = dest_pack
        .as_mut()
        .ok_or_else(|| "包句柄缺失 / pack handle missing".to_string())?;
    let metadata = fs::metadata(&task.src).map_err(|e| {
        format!(
            "读取源文件信息失败 / failed to stat source {}: {e}",
            task.src.display()
        )
    })?;
    progress.file_total = metadata.len();
    let name = task
        .src
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_string();
    let info = FileInfo::new(
        name,
        metadata.len(),
        FileFinder::get_file_modified(&metadata),
        FileKind::File { hash: None },
    );
    let mut run_buf = vec![0u8; 1024 * 1024];
    let outcome = common::copy_file_into_pack(
        pack,
        &mut run_buf,
        &info,
        &task.src,
        &task.dst,
        None,
        &mut |done, add, _info, _src, _dst| progress.add(add, done),
        false,
    );
    match outcome {
        CopyOutcome::Written | CopyOutcome::Unchanged => Ok(()),
        CopyOutcome::Failed => Err(format!(
            "复制到包失败 / failed to copy {} into pack",
            task.src.display()
        )),
    }
}

fn copy_out_of_pack(
    task: &FileTask,
    source_pack: &mut Option<ManagerSync>,
    progress: &mut ProgressCtx,
) -> Result<(), String> {
    let pack = source_pack
        .as_mut()
        .ok_or_else(|| "包句柄缺失 / pack handle missing".to_string())?;
    let mut input = pack
        .open_virtual_file(&task.src)
        .map_err(|e| format!("打开包内文件失败 / failed to open pack file: {e}"))?;
    let len = input.get_len();
    progress.file_total = len;
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&task.dst)
        .map_err(|e| {
            format!(
                "创建目标文件失败 / failed to create target {}: {e}",
                task.dst.display()
            )
        })?;
    let written = pump(&mut input, &mut output, progress)?;
    if written != len {
        return Err(format!(
            "文件长度不一致 / size mismatch: {written} != {len}"
        ));
    }
    Ok(())
}

fn copy_pack_file(
    task: &FileTask,
    source_pack: &mut Option<ManagerSync>,
    dest_pack: &mut Option<ManagerSync>,
    progress: &mut ProgressCtx,
) -> Result<(), String> {
    let source = source_pack
        .as_mut()
        .ok_or_else(|| "源包句柄缺失 / source pack handle missing".to_string())?;
    let dest = dest_pack
        .as_mut()
        .ok_or_else(|| "目标包句柄缺失 / destination pack handle missing".to_string())?;
    let mut input = source
        .open_virtual_file(&task.src)
        .map_err(|e| format!("打开包内文件失败 / failed to open pack file: {e}"))?;
    let len = input.get_len();
    progress.file_total = len;
    let modified = input.get_modified();
    let mut output = dest
        .virtual_file_options()
        .write(true)
        .create_new(true)
        .alloc_size(Some(len))
        .open(&task.dst)
        .map_err(|e| format!("创建包内文件失败 / failed to create pack file: {e}"))?;
    output
        .set_len(len)
        .map_err(|e| format!("设置包内文件长度失败 / failed to set pack file length: {e}"))?;
    let written = pump(&mut input, &mut output, progress)?;
    if written != len {
        return Err(format!(
            "文件长度不一致 / size mismatch: {written} != {len}"
        ));
    }
    let _ = output.set_modified(modified);
    Ok(())
}

fn pump<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    progress: &mut ProgressCtx,
) -> Result<u64, String> {
    let mut buf = vec![0u8; 1024 * 1024];
    let mut written = 0u64;
    loop {
        let read = reader
            .read(&mut buf)
            .map_err(|e| format!("读取失败 / read failed: {e}"))?;
        if read == 0 {
            break;
        }
        writer
            .write_all(&buf[..read])
            .map_err(|e| format!("写入失败 / write failed: {e}"))?;
        written = written.saturating_add(read as u64);
        progress.add(read as u64, written);
    }
    Ok(written)
}

fn discover(
    source: &TransferEndpoint,
    dest: &TransferEndpoint,
    rel: &Path,
    files: &mut Vec<FileTask>,
    dirs: &mut Vec<PathBuf>,
    tx: &Sender<JobEvent>,
    warnings: &mut u64,
) -> Result<(), String> {
    match source {
        TransferEndpoint::Local(src_root) => {
            let src_dir = src_root.join(rel);
            let entries = fs::read_dir(&src_dir).map_err(|e| {
                format!(
                    "读取源目录失败 / failed to read source directory {}: {e}",
                    src_dir.display()
                )
            })?;
            for entry in entries {
                let entry = entry
                    .map_err(|e| format!("读取目录项失败 / failed to read directory entry: {e}"))?;
                let file_type = entry
                    .file_type()
                    .map_err(|e| format!("读取文件类型失败 / failed to read file type: {e}"))?;
                let name = entry.file_name();
                let child_rel = rel.join(&name);
                let child_src = src_dir.join(&name);
                if file_type.is_dir() {
                    let _ = tx.send(JobEvent::Dir {
                        path: child_src.clone(),
                    });
                    dirs.push(child_rel.clone());
                    discover(source, dest, &child_rel, files, dirs, tx, warnings)?;
                } else if file_type.is_file() {
                    let metadata = entry.metadata().map_err(|e| {
                        format!(
                            "读取源文件信息失败 / failed to stat {}: {e}",
                            child_src.display()
                        )
                    })?;
                    let _ = tx.send(JobEvent::File {
                        path: child_src.clone(),
                    });
                    files.push(FileTask {
                        src: child_src,
                        dst: dest.path().join(&child_rel),
                        size: metadata.len(),
                    });
                } else {
                    *warnings = warnings.saturating_add(1);
                }
            }
        }
        TransferEndpoint::Pack {
            pack,
            path: src_root,
        } => {
            let mut pack = pack.clone();
            let src_dir = src_root.join(rel);
            let names = if src_dir.as_os_str().is_empty() {
                pack.get_root_struct_item_name_list()
                    .map_err(|e| format!("读取包根目录失败 / failed to read pack root: {e}"))?
            } else {
                pack.get_struct_item_name_list(&src_dir)
                    .map_err(|e| format!("读取包目录失败 / failed to read pack directory: {e}"))?
            };
            for name in names {
                let child_rel = rel.join(&name);
                let child_src = src_dir.join(&name);
                let item = pack.get_pack_struct_item(&child_src).map_err(|e| {
                    format!(
                        "读取包内条目失败 / failed to read pack entry {}: {e}",
                        child_src.display()
                    )
                })?;
                match item.item_type() {
                    PackStructItemType::Dir { .. } => {
                        let _ = tx.send(JobEvent::Dir {
                            path: child_src.clone(),
                        });
                        dirs.push(child_rel.clone());
                        discover(source, dest, &child_rel, files, dirs, tx, warnings)?;
                    }
                    PackStructItemType::File { .. } => {
                        let _ = tx.send(JobEvent::File {
                            path: child_src.clone(),
                        });
                        files.push(FileTask {
                            src: child_src,
                            dst: dest.path().join(&child_rel),
                            size: metadata_len(&item),
                        });
                    }
                }
            }
        }
    }
    Ok(())
}

fn create_dest_dir(endpoint: &TransferEndpoint, path: &Path) -> Result<(), String> {
    match endpoint {
        TransferEndpoint::Local(_) => fs::create_dir_all(path).map_err(|e| {
            format!(
                "创建目标目录失败 / failed to create target directory {}: {e}",
                path.display()
            )
        }),
        TransferEndpoint::Pack { pack, .. } => {
            let mut pack = pack.clone();
            pack.create_dir_all(&path).map_err(|e| {
                format!(
                    "创建包内目录失败 / failed to create pack directory {}: {e}",
                    path.display()
                )
            })
        }
    }
}

fn endpoint_is_dir(endpoint: &TransferEndpoint) -> Result<bool, String> {
    match endpoint {
        TransferEndpoint::Local(path) => fs::symlink_metadata(path)
            .map(|metadata| metadata.is_dir())
            .map_err(|e| {
                format!(
                    "读取源路径信息失败 / failed to stat source {}: {e}",
                    path.display()
                )
            }),
        TransferEndpoint::Pack { pack, path } => {
            let mut pack = pack.clone();
            let item = pack
                .get_pack_struct_item(path)
                .map_err(|e| format!("读取包内条目失败 / failed to read pack entry: {e}"))?;
            Ok(matches!(item.item_type(), PackStructItemType::Dir { .. }))
        }
    }
}

fn endpoint_file_len(endpoint: &TransferEndpoint) -> Result<u64, String> {
    match endpoint {
        TransferEndpoint::Local(path) => {
            fs::metadata(path)
                .map(|metadata| metadata.len())
                .map_err(|e| {
                    format!(
                        "读取源文件信息失败 / failed to stat source {}: {e}",
                        path.display()
                    )
                })
        }
        TransferEndpoint::Pack { pack, path } => {
            let mut pack = pack.clone();
            let item = pack
                .get_pack_struct_item(path)
                .map_err(|e| format!("读取包内条目失败 / failed to read pack entry: {e}"))?;
            Ok(metadata_len(&item))
        }
    }
}

fn delete_source(endpoint: &TransferEndpoint) -> Result<(), String> {
    match endpoint {
        TransferEndpoint::Local(path) => {
            let metadata = fs::symlink_metadata(path).map_err(|e| {
                format!(
                    "读取源路径信息失败 / failed to stat source {}: {e}",
                    path.display()
                )
            })?;
            if metadata.is_dir() {
                fs::remove_dir_all(path)
            } else {
                fs::remove_file(path)
            }
            .map_err(|e| {
                format!(
                    "删除源路径失败 / failed to delete source {}: {e}",
                    path.display()
                )
            })
        }
        TransferEndpoint::Pack { pack, path } => {
            let mut pack = pack.clone();
            let item = pack
                .get_pack_struct_item(path)
                .map_err(|e| format!("读取包内条目失败 / failed to read pack entry: {e}"))?;
            if matches!(item.item_type(), PackStructItemType::Dir { .. }) {
                pack.delete_dir_all(path)
            } else {
                pack.delete_file(path)
            }
            .map_err(|e| {
                format!(
                    "删除包内路径失败 / failed to delete pack entry {}: {e}",
                    path.display()
                )
            })
        }
    }
}
//AI_END===

fn metadata_len(item: &PackStructItem) -> u64 {
    match item.metadata() {
        PackFileMetadataRun::Loaded(metadata) => metadata.len(),
        _ => 0,
    }
}

#[cfg(test)]
mod test;
