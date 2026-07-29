use crate::command::create_pb;
use clap::Args;
use clap::error::Result;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::fs::File;
use std::io::Write;
use std::sync::mpsc;
use std::{error, io, thread};
use tracing::{error, info};
use water_ball_tool::file_finder::{FileFinder, FilesList, SearchResult};

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
}

impl FileFinderArgs {
    #[cfg(test)]
    pub(crate) fn new(path: String, out_path: Option<String>, skip_symlinks: bool) -> Self {
        Self {
            path,
            out_path,
            skip_symlinks,
        }
    }
}

/// 文件查找器——多线程扫描目录并输出 JSON 文件列表。
///
/// File finder — multi-threaded directory scanner that outputs a JSON file list.
pub fn ff(args: FileFinderArgs, mp: Option<&MultiProgress>) -> Result<(),Box<dyn error::Error>> {
    //进度条
    let pb = create_pb(mp);

    let files_list = search_files(&args.path, args.skip_symlinks, Option::from(&pb)).unwrap();

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

const SEARCH_MAX_THREAD_COUNT: usize = 16;

/// 搜索文件并显示进度 / Search files with progress display
pub(crate) fn search_files(
    path: &str,
    skip_symlink: bool,
    pb: Option<&ProgressBar>,
) -> io::Result<FilesList> {
    if let Some(pb) = &pb {
        pb.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.green} {msg} ({pos} 个文件和目录)")
                .unwrap(),
        );
        pb.set_message("搜索文件中");
    }
    let mut file_count = 0;
    let mut dir_count = 0;

    //搜索
    let ff = FileFinder;
    if let Some(pb) = &pb {
        let (tx, rx) = mpsc::channel();
        let (rtx, rrx) = mpsc::channel();
        let t_path = path.to_string();
        thread::spawn(move || {
            rtx.send(ff.search(t_path.as_ref(), skip_symlink, tx, SEARCH_MAX_THREAD_COUNT))
                .expect("线程发送结果错误");
        });
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
        rrx.recv()
            .expect("无法获取结果")
            .map(SearchResult::into_files_list)
    } else {
        let (tx, _) = mpsc::channel();
        ff.search(path.as_ref(), skip_symlink, tx, SEARCH_MAX_THREAD_COUNT)
            .map(SearchResult::into_files_list)
    }
}
