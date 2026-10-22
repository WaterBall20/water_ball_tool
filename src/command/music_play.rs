use crate::command::music_play::play_list::{PlayList, Track};
use crate::command::music_play::runtime::Runtime;
use crate::multi_progress_init;
use clap::{Args, ValueEnum};
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::prelude::Accessor;
use std::error;
use std::path::{Path, PathBuf};
use tracing::{debug, error, info, warn};

mod decoder;
pub mod effect;
mod play_list;
mod player;
mod runtime;
pub mod tui;

#[derive(Args, Debug)]
pub(crate) struct MusicPlayArgs {
    ///播放文件或目录
    files: Vec<PathBuf>,
    ///初始音量 (0到<1的小数或0-100的数，大于或等于1视为0-100)
    #[arg(short, long, default_value_t = 0.75)]
    volume: f32,
    ///详细输出
    #[arg(long)]
    verbose: bool,
    /// 播放模式
    #[arg(short, long, default_value = "sequential-loop")]
    mode: LoopMode,
}

#[derive(ValueEnum, Clone, Debug)]
pub(crate) enum LoopMode {
    /// 顺序循环
    SequentialLoop,
    /// 顺序播放
    Sequential,
    /// 单曲循环
    RepeatOne,
    /// 随机播放
    Shuffle,
    /// 周期不重复
    PeriodicShuffle,
}

pub fn args(args: MusicPlayArgs) -> Result<(), Box<dyn error::Error>> {
    let mp = multi_progress_init();
    let files = args.files;
    let volume = {
        if args.volume < 0.0 {
            panic!("参数错误：声音参数不能小于0")
        } else if args.volume > 100.0 {
            panic!("参数错误：声音参数不能大于100")
        }
        args.volume
    };
    let loop_mode = args.mode;
    info!("开始搜集音频信息");
    let play_list = create_play_list(&files, loop_mode)?;
    let mut runtime = match Runtime::create(play_list, &mp) {
        Ok(runtime) => runtime,
        Err(e) => {
            return Err(format!("创建运行时失败：{e:?}").into());
        }
    };
    runtime
        .next()
        .unwrap_or_else(|e| error!("初始化播放发生错误： {e}"));
    runtime.set_volume(volume);

    //交互处理
    loop {
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        match input.trim() {
            "next" | "n" => {
                debug!("开始执行换曲");
                runtime.next().unwrap_or_else(|e| {
                    error!("换曲发生错误: err:{e}");
                });
            }
            "Play" | "P" => {
                debug!("播放");
                runtime.play().unwrap_or_else(|e| {
                    error!("播放发生错误：err: {e}");
                });
            }
            "pause" | "p" => {
                debug!("暂停");
                runtime.pause().unwrap_or_else(|e| {
                    error!("暂停发生错误: err: {e}");
                });
            }
            "exit" => {
                debug!("退出");
                runtime.run_stop().unwrap_or_else(|e| {
                    error!("停止播放发生错误: err: {e:?}");
                });
                break;
            }
            _ => warn!("未知命令： {input}"),
        }
    }
    Ok(())
}

fn create_play_list(
    files: &[PathBuf],
    loop_mode: LoopMode,
) -> Result<PlayList, Box<dyn error::Error>> {
    fn dir_tracks(path: &Path, tracks: &mut Vec<Track>) {
        let read_dir = path.read_dir();
        match read_dir {
            Ok(dir) => {
                for i in dir {
                    match i {
                        Ok(entry) => {
                            let path = entry.path();
                            if path.is_dir() {
                                dir_tracks(&path, tracks);
                            } else if path.is_file() {
                                let track = create_track(&path);
                                match track {
                                    Ok(track) => {
                                        tracks.push(track);
                                    }
                                    Err(e) => {
                                        error!(r#"创建文件"{}"轨道失败, err: {e}"#, path.display());
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            error!(r#"迭代目录"{}"发生错误, err: {e}"#, path.display());
                        }
                    }
                }
            }
            Err(e) => {
                error!(r#"无法读取目录:"{}", err,: {e}"#, path.display());
            }
        }
    }
    let mut tracks = Vec::new();
    for path in files {
        if !path.exists() {
            error!(r#"文件"{}"不存在"#, path.display());
            continue;
        }
        if path.is_file() {
            let track = create_track(path);
            match track {
                Ok(track) => {
                    tracks.push(track);
                }
                Err(e) => {
                    error!(r#"创建文件"{}"轨道失败, err: {e}"#, path.display());
                }
            }
        } else if path.is_dir() {
            dir_tracks(path, &mut tracks);
        }
    }
    if tracks.is_empty() {
        Err(Box::new(std::io::Error::other("轨道为空")))?;
    }
    Ok(PlayList::new(tracks, loop_mode))
}

fn create_track(path: &Path) -> Result<Track, Box<dyn error::Error>> {
    let tagged = lofty::read_from_path(path);
    match tagged {
        Ok(tagged) => {
            let duration = tagged.properties().duration();
            // 3. 获取主要标签（通常是 ID3v2 或 Vorbis Comments）
            if let Some(tag) = tagged.primary_tag() {
                // 4. 读取标题，若标签缺失则使用文件名作为后备
                let title = tag
                    .title()
                    .as_deref()
                    .unwrap_or_else(|| {
                        path.file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("Unknown")
                    })
                    .to_string();

                // 5. 读取艺术家，缺失则标记为 Unknown
                let artist = tag.artist().as_deref().unwrap_or("Unknown").to_string();
                Ok(Track::new(
                    path.to_path_buf(),
                    title,
                    artist,
                    duration,
                    tagged.file_type(),
                ))
            } else {
                Ok(Track::new(
                    path.to_path_buf(),
                    path.file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("Unknown")
                        .to_string(),
                    "Unknown".to_string(),
                    duration,
                    tagged.file_type(),
                ))
            }
        }
        Err(e) => {
            error!(r#"获取文件"{}"，的音频信息失败"#, path.display());
            Err(Box::new(e))
        }
    }
}
