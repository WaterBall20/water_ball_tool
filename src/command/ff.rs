use crate::command::create_pb;
use clap::Args;
use clap::error::Result;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::num::NonZero;
use std::path::PathBuf;
use std::sync::mpsc;
use std::{error, io, thread};
use tracing::{error, info, warn};
use water_ball_tool::file_finder::{
    FileFinder, FileInfo, FilesList, SearchEvent, SearchWarningType,
};

/// `ff` 子命令的参数 / Arguments for the `ff` subcommand
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
}

impl FileFinderArgs {
    #[cfg(test)]
    pub(crate) fn new(
        path: String,
        out_path: Option<String>,
        skip_symlinks: bool,
        thread_count: Option<usize>,
    ) -> Self {
        Self {
            path,
            out_path,
            skip_symlinks,
            thread_count,
        }
    }
}

/// 文件查找器——多线程扫描目录并输出 JSON 文件列表。
///
/// File finder — multi-threaded directory scanner that outputs a JSON file list.
pub fn ff(args: FileFinderArgs, mp: Option<&MultiProgress>) -> Result<(), Box<dyn error::Error>> {
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

    let files_list =
        search_files(&args.path, args.skip_symlinks, pb.as_ref(), thread_count).unwrap();

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
    pb: Option<&ProgressBar>,
    thread_count: usize,
) -> io::Result<FilesList> {
    if let Some(pb) = &pb {
        pb.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.green} {msg} ({pos} 个文件和目录)")
                .unwrap(),
        );
        pb.set_message("搜索文件中");
    }

    //搜索
    let ff = FileFinder;
    let (tx, rx) = mpsc::channel();
    let (ff_thread, event_rx) = ff.search_stream(&path, skip_symlink, tx, thread_count)?;

    //获取结果线程
    let mut results = HashMap::<PathBuf, FileInfo>::new();
    let r = thread::spawn(move || {
        for event in event_rx {
            match event {
                SearchEvent::Entry(p, i) => {
                    results.insert(p, i);
                }
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
        results
    });

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
    ff_thread
        .join()
        .map_err(|e| io::Error::other(format!("搜索时发生错误: {e:?}")))??;
    let r = r
        .join()
        .map_err(|e| io::Error::other(format!("获取搜索结果时发生错误: {e:?}")))?;
    Ok(FileFinder::build_tree(&r, path.as_ref()))
}
