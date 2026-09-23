use crate::command::{BUF_LEN, PACK_PROGRESS_STYLE_TEMPLATE, SPINNER_TEMPLATE, create_pb};
use clap::error::Result;
use clap::{Args, ValueEnum};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use md5::Md5;
use md5::digest::FixedOutputReset;
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha512};
use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::mem::replace;
use std::num::NonZero;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread::JoinHandle;
use std::{error, io, thread};
use tracing::{error, info, warn};
use water_ball_tool::file_finder::{
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
    #[arg(short, long, value_terminator = ";")]
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
pub fn args(args: FileFinderArgs, mp: Option<&MultiProgress>) -> Result<(), Box<dyn error::Error>> {
    //进度条
    let pb = create_pb(mp);
    //所用的线程数
    let thread_count = if let Some(v) = args.thread_count {
        v
    } else {
        let thread_count = thread::available_parallelism()
            .unwrap_or(NonZero::new(8).unwrap())
            .get();
        info!("未指定线程数量，将使用{thread_count}线程");
        thread_count
    };

    let hash_type = args.hash.map(|hash| HashTypeS::new(&hash));

    let files_list = search_files(
        &args.path,
        args.skip_symlinks,
        pb,
        thread_count,
        hash_type,
        mp,
    )
    .unwrap();

    //输出到输出文件(若存在参数)
    if let Some(out_path) = args.out_path {
        //只写模式打开文件，
        match File::create(&out_path) {
            Ok(mut f) => {
                info!("正在将搜索结果输出到文件");
                let data = serde_json::to_vec_pretty(&files_list).expect("数据转换错误");
                f.write_all(&data).expect("保存到文件错误");
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
pub(crate) fn search_files(
    path: &str,
    skip_symlink: bool,
    pb: Option<ProgressBar>,
    thread_count: usize,
    hash_type: Option<HashTypeS>,
    mp: Option<&MultiProgress>,
) -> io::Result<FilesList> {
    if let Some(pb) = &pb {
        pb.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.green} {msg} ({pos} 个文件和目录)")
                .unwrap(),
        );
        pb.set_message("搜索文件中");
    }
    let thread_count = if hash_type.is_some() {
        thread_count / 2
    } else {
        thread_count
    };
    let queue = Arc::new((Mutex::new(VecDeque::new()), Condvar::new()));
    let file_count = Arc::new(AtomicU64::new(0));
    let data_len = Arc::new(AtomicU64::new(0));
    let search_end = Arc::new(AtomicBool::new(false));

    //搜索
    let ff = FileFinder;
    let (tx, rx) = mpsc::channel();
    let (ff_thread, event_rx) = ff.search_stream(path, skip_symlink, tx, thread_count)?;

    //获取结果线程
    let results = Arc::new(Mutex::new(HashMap::new()));
    let ff_r_thread = {
        let hash_type = hash_type.clone();
        let file_count = file_count.clone();
        let queue = queue.clone();
        let results = results.clone();
        let data_len = data_len.clone();

        let search_end = search_end.clone();
        thread::spawn(move || {
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
                                queue
                                    .0
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                                    .push_back((p, i));
                                queue.1.notify_one();
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
        })
    };
    let ff_info_thread = thread::spawn(move || {
        let mut file_count = 0;
        let mut dir_count = 0;
        for (add_file, add_dir) in rx {
            file_count += add_file;
            dir_count += add_dir;
            if let Some(pb) = &pb {
                let all_count = file_count + dir_count;
                pb.set_position(all_count);
                //10的倍数才更新
                if all_count.is_multiple_of(10) {
                    pb.set_message(format!("已发现 {file_count} 文件和 {dir_count} 个目录"));
                }
            }
        }
    });

    //哈希计算
    if let Some(hash_type) = hash_type {
        hash(
            &queue,
            &search_end,
            mp,
            thread_count,
            file_count,
            data_len,
            hash_type,
            results,
        );
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
    queue: &Arc<(Mutex<VecDeque<(PathBuf, FileInfo)>>, Condvar)>,
    search_end: &Arc<AtomicBool>,
    mp: Option<&MultiProgress>,
    thread_count: usize,
    all_file_count: Arc<AtomicU64>,
    data_len: Arc<AtomicU64>,
    hash_type: HashTypeS,
    results: Arc<Mutex<HashMap<PathBuf, FileInfo>>>,
) {
    // 总进度条 / Main progress bar
    let main_pb = create_pb(mp);
    if let Some(pb) = &main_pb {
        pb.set_length(0);
        pb.set_style(
            ProgressStyle::default_bar()
                .template(PACK_PROGRESS_STYLE_TEMPLATE)
                .unwrap()
                .progress_chars("=>-"),
        );
        pb.set_prefix("哈希计算总进度");
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

    for _ in 0..thread_count {
        let hash_type = hash_type.clone();
        hash_work(
            queue,
            search_end,
            &tx,
            &mut worker_pbs,
            &mut handles,
            hash_type,
            results.clone(),
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
                pb.set_length(data_len.load(Ordering::SeqCst));
                pb.set_position(written_len);
                pb.set_message(format!(
                    "[{file_count}/{}个文件]",
                    all_file_count.load(Ordering::SeqCst)
                ));
            }
            last_pb_update_len = written_len;
        }
    }

    for h in handles {
        h.join().expect("等待线程发生错误");
    }
}

fn hash_work(
    queue: &Arc<(Mutex<VecDeque<(PathBuf, FileInfo)>>, Condvar)>,
    search_end: &Arc<AtomicBool>,
    tx: &Sender<u64>,
    worker_pbs: &mut Vec<Option<ProgressBar>>,
    handles: &mut Vec<JoinHandle<()>>,
    mut hash_type: HashTypeS,
    results: Arc<Mutex<HashMap<PathBuf, FileInfo>>>,
) {
    use std::thread;
    let queue = queue.clone();
    let all_done = search_end.clone();
    let tx = tx.clone();
    let w_pb = worker_pbs.pop().unwrap();

    handles.push(thread::spawn(move || {
        let mut read_buf = vec![0u8; BUF_LEN];
        loop {
            // 1. 从队列取路径（空队列时 Condvar 挂起）
            //    Pop path from queue (Condvar suspend when empty)
            let (path, info) = {
                let (lock, cvar) = &*queue;
                let mut q = lock.lock().unwrap();
                loop {
                    if let Some(p) = q.pop_front() {
                        break p;
                    }
                    if all_done.load(Ordering::SeqCst) {
                        cvar.notify_all();
                        return;
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
                    continue;
                }
            };

            if let Some(pb) = &w_pb {
                pb.set_length(info.length());
                pb.set_position(0);
                pb.set_message(format!("正在计算哈希: {}", path.display()));
            }

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

                        if let Some(pb) = &w_pb
                            && read_len - pb_last_up_len > 10 * 1024 * 1024
                        {
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

            //
            if let Some(pb) = &w_pb {
                pb.set_position(info.length());
            }
            if let Err(e) = tx.send(info.length()) {
                error!("发送更新总进度失败, err: {e}");
                continue;
            }

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
    }));
}
