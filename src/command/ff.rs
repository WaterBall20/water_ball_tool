use crate::command::{BUF_LEN, PACK_PROGRESS_STYLE_TEMPLATE, SPINNER_TEMPLATE, create_pb};
use crate::multi_progress_init;
use clap::error::Result;
use clap::{Args, ValueEnum};
//AI=== 样式改由 crate::command::progress_style 构造，移除不再使用的 ProgressStyle 导入 / styles now come from crate::command::progress_style, drop unused ProgressStyle import
use indicatif::{MultiProgress, ProgressBar};
//AI_END===
use md5::Md5;
use md5::digest::FixedOutputReset;
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha512};
use std::collections::HashMap;
use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::mem::replace;
use std::num::NonZero;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::{error, io, thread};
use tracing::{error, info, warn};
use water_ball_tool_lib::concurrent_ring::ConcurrentRing;
use water_ball_tool_lib::file_finder::{
    FileFinder, FileInfo, FileKind, FilesList, SearchEvent, SearchWarningType,
};

/// 文件查找命令
#[derive(Args, Debug)]
pub(crate) struct FileFinderArgs {
    /// 搜索的目录路径 / Directory path to search
    path: String,
    /// 输出的 JSON 文件路径（可选，不提供则打印到日志）/ Output JSON file path (optional; prints to log if omitted)
    out_path: Option<String>,
    /// 跳过符号链接 / Skip symlinks
    #[arg(short, long)]
    skip_symlinks: bool,
    ///线程数量
    #[arg(short, long)]
    thread_count: Option<usize>,
    ///计算哈希值
    hash: Option<Vec<HashType>>,
}

impl FileFinderArgs {
    #[cfg(test)]
    pub(crate) fn new(
        path: String,
        out_path: Option<String>,
        skip_symlinks: bool,
        thread_count: Option<usize>,
        hash: Option<Vec<HashType>>,
    ) -> Self {
        Self {
            path,
            out_path,
            skip_symlinks,
            thread_count,
            hash,
        }
    }
}

#[derive(Clone, Debug, ValueEnum)]
pub(crate) enum HashType {
    #[value(alias = "*")]
    All,
    #[value(alias = "m5")]
    Md5,
    #[value(alias = "s256")]
    Sha256,
    #[value(alias = "s512")]
    Sha512,
    #[value(alias = "s1")]
    Sha1,
    #[value(alias = "b3")]
    Blake3,
    #[value(alias = "b2b")]
    Blake2b,
    #[value(alias = "crc")]
    Crc32,
}

#[derive(Clone)]
struct HashTypeS {
    sha256: Option<Sha256>,
    sha512: Option<Sha512>,
    blake3: Option<blake3::Hasher>,
    blake2b: Option<blake2b_simd::State>,
    crc32: Option<crc32fast::Hasher>,
    sha1: Option<Sha1>,
    md5: Option<Md5>,
}

impl HashTypeS {
    fn new(hash_list: &[HashType]) -> Self {
        let mut this = Self {
            sha256: None,
            sha512: None,
            blake3: None,
            blake2b: None,
            crc32: None,
            sha1: None,
            md5: None,
        };
        for i in hash_list {
            match i {
                HashType::All => {
                    return Self {
                        sha256: Some(Sha256::default()),
                        sha512: Some(Sha512::default()),
                        blake3: Some(blake3::Hasher::new()),
                        blake2b: Some(blake2b_simd::State::new()),
                        crc32: Some(crc32fast::Hasher::new()),
                        sha1: Some(Sha1::default()),
                        md5: Some(Md5::default()),
                    };
                }
                HashType::Sha256 => this.sha256 = Some(Sha256::default()),
                HashType::Sha512 => this.sha512 = Some(Sha512::default()),
                HashType::Blake3 => this.blake3 = Some(blake3::Hasher::new()),
                HashType::Blake2b => this.blake2b = Some(blake2b_simd::State::new()),
                HashType::Crc32 => this.crc32 = Some(crc32fast::Hasher::new()),
                HashType::Sha1 => {
                    this.sha1 = Some(Sha1::default());
                    warn!("已启用SHA-1，但SHA-1不安全");
                }
                HashType::Md5 => {
                    this.md5 = Some(Md5::default());
                    warn!("已启用MD5，但MD5不安全");
                }
            }
        }
        this
    }

    fn update(&mut self, data: &[u8]) {
        //SHA-256
        if let Some(h) = &mut self.sha256 {
            h.update(data);
        }
        //SHA-512
        if let Some(h) = &mut self.sha512 {
            h.update(data);
        }
        //BLAKE3
        if let Some(h) = &mut self.blake3 {
            h.update(data);
        }
        //BLAKE2b
        if let Some(h) = &mut self.blake2b {
            h.update(data);
        }
        //CRC32
        if let Some(h) = &mut self.crc32 {
            h.update(data);
        }
        //SHA-1
        if let Some(h) = &mut self.sha1 {
            h.update(data);
        }
        //MD5
        if let Some(h) = &mut self.md5 {
            h.update(data);
        }
    }

    fn reset(&mut self) {
        //SHA-256
        if let Some(h) = &mut self.sha256 {
            h.reset();
        }
        //SHA-512
        if let Some(h) = &mut self.sha512 {
            h.reset();
        }
        //BLAKE3
        if let Some(h) = &mut self.blake3 {
            h.reset();
        }
        //BLAKE2b
        if let Some(h) = &mut self.blake2b {
            *h = blake2b_simd::State::new();
        }
        //CRC32
        if let Some(h) = &mut self.crc32 {
            h.reset();
        }
        //SHA-1
        if let Some(h) = &mut self.sha1 {
            h.reset();
        }
        //MD5
        if let Some(h) = &mut self.md5 {
            h.reset();
        }
    }

    fn finalize_reset_str_map(&mut self) -> HashMap<String, String> {
        const T: &[u8; 16] = b"0123456789abcdef";
        #[inline]
        fn to_hex(data: &[u8]) -> Vec<char> {
            let mut s = Vec::with_capacity(data.len() * 2);
            for &b in data {
                s.push(T[(b >> 4) as usize] as char);
                s.push(T[(b & 0xf) as usize] as char);
            }
            s
        }
        #[inline]
        fn to_str(hex: Vec<char>) -> String {
            let mut s = String::with_capacity(hex.len());
            for ch in hex {
                s.push(ch);
            }
            s
        }

        #[inline]
        fn fb_to_str(fb: &[u8]) -> String {
            to_str(to_hex(fb))
        }

        let mut r = HashMap::new();
        //SHA-256
        if let Some(h) = &mut self.sha256 {
            let fb = h.finalize_fixed_reset().to_vec();
            let s = fb_to_str(&fb);
            r.insert("sha-256", s);
        }
        //SHA-512
        if let Some(h) = &mut self.sha512 {
            let fb = h.finalize_fixed_reset().to_vec();
            let s = fb_to_str(&fb);
            r.insert("sha-512", s);
        }
        //BLAKE3
        if let Some(h) = &mut self.blake3 {
            let hf = h.finalize();
            let fb = hf.as_bytes();
            let s = fb_to_str(fb);
            r.insert("blake3", s);
            h.reset();
        }
        //BLAKE2b
        if let Some(h) = &mut self.blake2b {
            let hf = h.finalize();
            let fb = hf.as_bytes();
            let s = fb_to_str(fb);
            r.insert("blake2b", s);
            *h = blake2b_simd::State::new();
        }
        //CRC32
        if let Some(h) = &mut self.crc32 {
            let h = replace(h, crc32fast::Hasher::new());
            let hf = h.finalize();
            let fb = hf.to_be_bytes();
            let s = fb_to_str(&fb);
            r.insert("crc32", s);
        }
        //SHA-1
        if let Some(h) = &mut self.sha1 {
            let fb = h.finalize_fixed_reset().to_vec();
            let s = fb_to_str(&fb);
            r.insert("sha-1", s);
        }
        //MD5
        if let Some(h) = &mut self.md5 {
            let fb = h.finalize_fixed_reset().to_vec();
            let s = fb_to_str(&fb);
            r.insert("md5", s);
        }
        r.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
    }
}

/// 文件查找器——多线程扫描目录并输出 JSON 文件列表。
///
/// File finder — multi-threaded directory scanner that outputs a JSON file list.
pub fn args(args: FileFinderArgs) -> Result<(), Box<dyn error::Error>> {
    let mp = multi_progress_init("ff");
    //进度条
    let pb = create_pb(&mp);
    //所用的线程数
    //AI=== 用 const match 构造非零默认线程数，消除运行时 expect / build a non-zero default via const match, removing the runtime expect
    // 默认线程数 8 必然非零：在编译期用 const match 得到非零值，运行期不再需要 expect。
    // The default worker count 8 is always non-zero: a const match yields the non-zero value at
    // compile time, so no runtime expect is needed.
    const DEFAULT_THREAD_COUNT: NonZero<usize> = match NonZero::new(8) {
        Some(v) => v,
        None => NonZero::<usize>::MIN,
    };
    let thread_count = if let Some(v) = args.thread_count {
        v
    } else {
        let thread_count = thread::available_parallelism()
            .unwrap_or(DEFAULT_THREAD_COUNT)
            .get();
        info!("未指定线程数量，将使用{thread_count}线程");
        thread_count
    };
    //AI_END===

    let hash_type = args.hash.map(|hash| HashTypeS::new(&hash));

    let files_list = search_files(
        &args.path,
        args.skip_symlinks,
        pb,
        thread_count,
        hash_type,
        &mp,
    )?;

    //输出到输出文件(若存在参数)
    if let Some(out_path) = args.out_path {
        //只写模式打开文件，
        match File::create(&out_path) {
            Ok(mut f) => {
                info!("正在将搜索结果输出到文件");
                //AI=== 用 ? 传播序列化与写文件错误，替代会 panic 的 expect / propagate serialize and write errors with ?, replacing panicking expect
                let data = serde_json::to_vec_pretty(&files_list)?;
                f.write_all(&data)?;
                //AI_END===
                info!(r#"搜索结果已输出到文件: "{out_path}""#);
                Ok(())
            }
            Err(err) => {
                error!(r#"无法打开输出文件: "{out_path}" , Error: '{err}'"#);
                Err(err)?
            }
        }
    } else {
        info!("搜索结果：{files_list:?}");
        Ok(())
    } /**/
}

/// 搜索文件并显示进度 / Search files with progress display
fn search_files(
    path: &str,
    skip_symlink: bool,
    pb: ProgressBar,
    thread_count: usize,
    hash_type: Option<HashTypeS>,
    mp: &MultiProgress,
) -> Result<FilesList, Box<dyn error::Error>> {
    //初始化进度条
    //AI=== 模板非法时由 progress_style 记 error 并回退默认样式，不再 expect / progress_style logs an error and falls back to the default style, no expect
    pb.set_style(crate::command::progress_style(
        "{spinner:.green} {msg} ({pos} 个文件和目录)",
    ));
    //AI_END===
    pb.set_message("搜索文件中");
    let ff_thread_count = if hash_type.is_some() { 1 } else { thread_count };
    let ring_buf = ConcurrentRing::new();
    let file_count = Arc::new(AtomicU64::new(0));
    let data_len = Arc::new(AtomicU64::new(0));
    let search_end = Arc::new(AtomicBool::new(false));

    //搜索
    let ff = FileFinder;
    let (tx, rx) = mpsc::channel();
    let (ff_thread, event_rx) = ff.search_stream(path, skip_symlink, tx, ff_thread_count)?;

    //获取结果线程
    let results = Arc::new(Mutex::new(HashMap::new()));
    let ff_r_thread = {
        let hash_type = hash_type.clone();
        let file_count = file_count.clone();
        let ring_buf = ring_buf.clone();
        let results = results.clone();
        let data_len = data_len.clone();

        let search_end = search_end.clone();
        //AI=== 线程创建失败必须返回错误：改用 Builder::new().name(...).spawn(...)，失败经 ? 传播；
        // 禁止会 panic 的 std::thread::spawn。
        // Thread creation failure MUST be returned: use Builder::new().name(...).spawn(...) and
        // propagate the failure with `?`; the panicking std::thread::spawn is forbidden.
        let worker = move || {
            for event in event_rx {
                match event {
                    SearchEvent::Entry(p, i) => match i.file_kind() {
                        FileKind::Dir(_) => {
                            results
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .insert(p, i);
                        }
                        FileKind::File { .. } => {
                            if hash_type.is_some() {
                                file_count.fetch_add(1, Ordering::SeqCst);
                                data_len.fetch_add(i.length(), Ordering::SeqCst);
                                if let Err(e) = ring_buf.push_wait((p, i)) {
                                    error!("添加到缓冲区发生错误:{e}");
                                }
                            } else {
                                results
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                                    .insert(p, i);
                            }
                        }
                    },
                    SearchEvent::Warning(w) => match &w.warning_type {
                        SearchWarningType::PermissionDenied => {
                            warn!("权限不足：\tPath: {}", w.path.display());
                        }
                        SearchWarningType::ReadDirError(msg) => {
                            warn!("读取目录失败：\tMsg: {msg}, \tPath:{}", w.path.display());
                        }
                        SearchWarningType::MetadataError => {
                            warn!("读取元数据失败：\tPath: {}", w.path.display());
                        }
                        SearchWarningType::BrokenSymlink => {
                            warn!("符号链接断开：\tPath: {}", w.path.display());
                        }
                        SearchWarningType::InaccessibleEntry => {
                            warn!("读取条目：\tPath: {}", w.path.display());
                        }
                        SearchWarningType::ThreadError(msg) => {
                            error!("线程错误：\tMsg: {msg}, \tPath:{}", w.path.display());
                        }
                    },
                }
            }
            search_end.store(true, Ordering::SeqCst);
            results
        };
        thread::Builder::new()
            .name("ff-result-collector".to_string())
            .spawn(worker)?
        //AI_END===
    };
    //AI=== 线程创建失败必须返回错误：Builder + ? 传播，禁止会 panic 的 thread::spawn
    // Thread creation failure MUST be returned: Builder + `?`; the panicking thread::spawn is forbidden.
    let info_worker = move || {
        let mut file_count = 0;
        let mut dir_count = 0;
        for (add_file, add_dir) in rx {
            file_count += add_file;
            dir_count += add_dir;

            let all_count = file_count + dir_count;
            pb.set_position(all_count);
            //10的倍数才更新
            if all_count.is_multiple_of(10) {
                pb.set_message(format!("已发现 {file_count} 文件和 {dir_count} 个目录"));
            }
        }
    };
    let ff_info_thread = thread::Builder::new()
        .name("ff-search-progress".to_string())
        .spawn(info_worker)?;
    //AI_END===

    //哈希计算
    if let Some(hash_type) = hash_type {
        //AI=== 传播哈希线程 join 失败（原为 expect 恐慌）/ propagate hash worker join failures instead of expecting
        hash(
            ring_buf.clone(),
            &search_end,
            mp,
            thread_count,
            file_count,
            data_len,
            hash_type,
            results,
        )?;
        //AI_END===
    }
    ff_info_thread
        .join()
        .map_err(|e| io::Error::other(format!("搜索信息线程发生错误: {e:?}")))?;
    ff_thread
        .join()
        .map_err(|e| io::Error::other(format!("搜索时发生错误: {e:?}")))??;
    let r = ff_r_thread
        .join()
        .map_err(|e| io::Error::other(format!("获取搜索结果时发生错误: {e:?}")))?;
    Ok(FileFinder::build_tree(
        &r.lock().unwrap_or_else(std::sync::PoisonError::into_inner),
        path.as_ref(),
    ))
}

fn hash(
    ring_buf: ConcurrentRing<(PathBuf, FileInfo)>,
    search_end: &Arc<AtomicBool>,
    mp: &MultiProgress,
    thread_count: usize,
    all_file_count: Arc<AtomicU64>,
    data_len: Arc<AtomicU64>,
    hash_type: HashTypeS,
    results: Arc<Mutex<HashMap<PathBuf, FileInfo>>>,
    //AI=== 返回 io::Result，线程 join 失败可向上传播（原为 unit）/ return io::Result so worker join failures propagate (previously unit)
) -> io::Result<()> {
    //AI_END===
    // 总进度条 / Main progress bar
    let main_pb = create_pb(mp);
    main_pb.set_length(0);
    //AI=== 进度条样式改用 progress_style，模板非法时降级而非 panic / use progress_style so a bad template degrades instead of panicking
    main_pb.set_style(
        crate::command::progress_style(PACK_PROGRESS_STYLE_TEMPLATE).progress_chars("=>-"),
    );
    //AI_END===
    main_pb.set_prefix("哈希计算总进度");

    // 创建子线程进度条 / Create per-worker progress bars
    let mut worker_pbs = Vec::with_capacity(thread_count);
    for i in 0..thread_count {
        let pb = create_pb(mp);
        //AI=== 同上，子线程进度条样式不再 expect / same as above, per-worker bar style no longer expects
        pb.set_style(
            crate::command::progress_style(PACK_PROGRESS_STYLE_TEMPLATE).progress_chars("=>-"),
        );
        //AI_END===
        pb.set_prefix(format!("线程{i}"));
        worker_pbs.push(pb);
    }

    let (tx, rx) = mpsc::channel();
    let mut handles = Vec::with_capacity(thread_count);

    for _i in 0..thread_count {
        let hash_type = hash_type.clone();
        //AI=== 工作线程创建失败向上传播（原 thread::spawn 在 OS 无法创建线程时会 panic）
        // Propagate worker spawn failures (std::thread::spawn panics when the OS cannot create a thread).
        hash_work(
            ring_buf.clone(),
            search_end,
            &tx,
            &mut worker_pbs,
            &mut handles,
            hash_type,
            results.clone(),
        )?;
        //AI_END===
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
            main_pb.set_length(data_len.load(Ordering::SeqCst));
            main_pb.set_position(written_len);
            main_pb.set_message(format!(
                "[{file_count}/{}个文件]",
                all_file_count.load(Ordering::SeqCst)
            ));

            last_pb_update_len = written_len;
        }
    }

    for h in handles {
        //AI=== join 失败转为 io::Error 向上传播，替代 expect / turn join failures into io::Error instead of expecting
        h.join()
            .map_err(|e| io::Error::other(format!("哈希计算线程发生错误: {e:?}")))?;
        //AI_END===
    }
    Ok(())
}

//AI=== 返回 io::Result：线程创建失败必须向上传播（原为 unit）
// Returns io::Result so thread-creation failures propagate (previously unit).
fn hash_work(
    ring_buf: ConcurrentRing<(PathBuf, FileInfo)>,
    search_end: &Arc<AtomicBool>,
    tx: &Sender<u64>,
    worker_pbs: &mut Vec<ProgressBar>,
    handles: &mut Vec<JoinHandle<()>>,
    mut hash_type: HashTypeS,
    results: Arc<Mutex<HashMap<PathBuf, FileInfo>>>,
) -> io::Result<()> {
    //AI_END===
    use std::thread;
    let all_done = search_end.clone();
    let tx = tx.clone();
    //AI=== 逻辑错误：每个哈希工作线程在派发前都已创建并压入自己的进度条，pop 必然成功
    // 逻辑错误：工作线程数与其进度条数一一对应，`pop` 失败说明不变量被破坏。
    // Logic error: every hash worker is dispatched with its own pre-created progress bar; a failed
    // `pop` means the one-to-one invariant was broken.
    let pb = match worker_pbs.pop() {
        Some(pb) => pb,
        None => unreachable!(
            "逻辑错误：哈希工作线程缺少进度条 / logic error: hash worker has no progress bar"
        ),
    };
    //AI_END===

    //AI=== 线程创建失败必须返回错误：Builder::new().name(...).spawn(...) + ? 传播，禁止会 panic 的 thread::spawn
    // Thread creation failure MUST be returned: Builder::new().name(...).spawn(...) + `?`; thread::spawn is forbidden.
    let worker = move || {
        let mut read_buf = vec![0u8; BUF_LEN];
        loop {
            // 1. 从队列取路径（空队列时 Condvar 挂起）
            //    Pop path from queue (Condvar suspend when empty)
            let (path, info) = {
                if let Some(p) = ring_buf.pop() {
                    p
                } else {
                    //搜索完成
                    if all_done.load(Ordering::SeqCst) {
                        return;
                    }
                    //AI=== 挂起用 spinner 样式改走 progress_style，模板非法时降级 / suspended spinner style goes through progress_style, degrading on bad template
                    pb.set_style(crate::command::progress_style(SPINNER_TEMPLATE));
                    //AI_END===
                    pb.set_message("已挂起");
                    let r = match ring_buf.pop_wait() {
                        Ok(v) => v,
                        Err(e) => {
                            if let water_ball_tool_lib::concurrent_ring::ConcurrentRingErrorKind::SingleInstance =
                                e.kind
                            {
                                return;
                            }
                            error!("挂起时发生未知错误：{e}");
                            return;
                        }
                    };
                    //AI=== 恢复子线程进度条样式也走 progress_style，不再 expect / restore the per-worker bar style via progress_style, no expect
                    pb.set_style(
                        crate::command::progress_style(PACK_PROGRESS_STYLE_TEMPLATE)
                            .progress_chars("=>-"),
                    );
                    //AI_END===
                    r
                }
            };

            //尝试打开文件
            let mut in_file = match File::open(&path) {
                Ok(file) => file,
                Err(err) => {
                    match err.kind() {
                        ErrorKind::PermissionDenied => {
                            //权限不足
                            error!(
                                r#"无法打开文件"{}"，权限不足，将跳过，err:{err}"#,
                                path.display()
                            );
                        }
                        _ => error!(r#"[未处理错误]无法打开文件"{}"，err:{err}"#, path.display()),
                    }
                    //AI=== 通道断开时仅记录日志并防御性重置哈希状态，避免状态残留污染 / on a closed channel only log and defensively reset hash state so nothing leaks
                    if let Err(e) = tx.send(info.length()) {
                        error!("发送更新总进度失败, err: {e}");
                    }
                    // 打开失败时未读取任何数据，仍主动 reset，保证下一文件从干净状态开始。
                    // No data was read when opening failed; reset anyway so the next file starts clean.
                    hash_type.reset();
                    //AI_END===
                    continue;
                }
            };

            pb.set_length(info.length());
            pb.set_position(0);
            pb.set_message(format!("正在计算哈希: {}", path.display()));

            let mut read_len = 0;

            let mut pb_last_up_len = 0;
            let loop_no_err = loop {
                match in_file.read(&mut read_buf) {
                    Ok(0) => {
                        //EOF 防御：未达到记录长度时警告而不是进入无限循环
                        if read_len < info.length() {
                            warn!(
                                r#"文件"{}"提前结束（EOF），已写入{read_len}B，搜索时记录长度{}B"#,
                                path.display(),
                                info.length()
                            );
                            break false;
                        }
                        break true;
                    }
                    Ok(this_read_len) => {
                        let data = &read_buf[..this_read_len];
                        hash_type.update(data);
                        read_len += this_read_len as u64;

                        if read_len - pb_last_up_len > 10 * 1024 * 1024 {
                            pb.set_position(read_len);
                            pb_last_up_len = read_len;
                        }
                    }
                    Err(err) => {
                        error!(r#"读取文件"{}"失败，将跳过，err:{err}"#, path.display());
                        break false;
                    }
                }
            };
            pb.set_position(info.length());
            //AI=== 修复哈希状态污染：发送失败不再 continue，保证后续 finalize/reset 必然执行 / fix hash-state pollution: never continue on send failure, so finalize/reset always runs
            if let Err(e) = tx.send(info.length()) {
                error!("发送更新总进度失败, err: {e}");
            }
            //AI_END===

            if loop_no_err {
                let hash_list = hash_type.finalize_reset_str_map();
                let new_info = FileInfo::new(
                    info.name().to_string(),
                    info.length(),
                    info.modified_time(),
                    match info.file_kind() {
                        FileKind::Dir(_) => unreachable!("逻辑错误: 目录不应进入哈希计算"),
                        FileKind::File { .. } => FileKind::File {
                            hash: Some(hash_list),
                        },
                    },
                );
                results
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(path, new_info);
            } else {
                hash_type.reset();
                results
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(path, info);
            }
        }
    };
    //工作线程编号取自已派发句柄数，避免新增参数 / worker index comes from the dispatched handle count, avoiding a new argument
    let worker_index = handles.len();
    handles.push(
        thread::Builder::new()
            .name(format!("ff-hash-worker-{worker_index}"))
            .spawn(worker)?,
    );
    Ok(())
    //AI_END===
}
