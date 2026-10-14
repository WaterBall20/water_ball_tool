use crate::command::music_play_test::audio::Audio;
use crate::command::music_play_test::decoder::{
    DecoderEventChannelSink, DecoderEventSink, DecoderEventType, DecoderResult,
};
use crate::command::music_play_test::play_list::{PlayList, Track};
use clap::{Args, ValueEnum};
use cpal::SAMPLE_RATE_48K;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use indicatif::MultiProgress;
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::prelude::Accessor;
use std::error;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tracing::{debug, error, info, warn};
use water_ball_tool_lib::ring_buffer_wait::RingBufferWait;

mod audio;
mod decoder;
mod play_list;
mod running;

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
    mode: crate::command::music_play::LoopMode,
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

pub fn args(args: MusicPlayArgs, mp: Option<MultiProgress>) -> Result<(), Box<dyn error::Error>> {
    let files = args.files;
    let volume = {
        if args.volume < 0.0 {
            panic!("参数错误：声音参数不能小于0")
        } else if args.volume > 100.0 {
            panic!("参数错误：声音参数不能大于100")
        }
        Arc::new(AtomicU32::new(args.volume.to_bits()))
    };
    let loop_mode = args.mode;
    info!("开始搜集音频信息");
    let next_id = Arc::new(AtomicUsize::new(0));
    let play_list = create_play_list(&files, loop_mode)?;
    let play_list = Arc::new(Mutex::new(play_list));

    let mut audio = Audio::new()?;
    next_inner(play_list.clone(), false, next_id.clone(), audio.clone()).unwrap_or_else(|e| {
        error!("初始化歌曲播放失败： {e:?}");
    });

    //交互处理
    loop {
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        match input.trim() {
            "next" | "n" => {
                debug!("开始执行换曲");
                next_inner(play_list.clone(), false, next_id.clone(), audio.clone())
                    .unwrap_or_else(|e| {
                        error!("换曲发生错误");
                    });
            }
            "Play" | "P" => {
                debug!("播放");
                if let Err(e) = audio.play() {
                    error!("播放失败：err: {e}");
                }
            }
            "pause" | "p" => {
                debug!("暂停");
                if let Err(e) = audio.pause() {
                    error!("暂停失败: err: {e}");
                }
            }
            "exit" => {
                debug!("退出");
                audio.stop().unwrap_or_else(|e| {
                    error!("停止播放发生错误");
                });
                break;
            }
            _ => warn!("未知命令： {input}"),
        }
    }
    Ok(())
}

fn next_inner(
    play_list: Arc<Mutex<PlayList>>,
    play_end: bool,
    next_id: Arc<AtomicUsize>,
    audio: Audio,
) -> DecoderResult<()> {
    let mut audio = audio;
    let mut play_list_lock = play_list.lock().unwrap_or_else(|err| err.into_inner());
    let track = play_list_lock.next(play_end, next_id.load(Ordering::Acquire));
    if let Some(t) = track {
        info!("换曲： {:?}", t.track());
        next_id.store(t.next_id(), Ordering::Release);
        let mut decoder =
            decoder::Decoder::new(audio.ring_buffer().clone(), t.track().path().to_path_buf())?;

        let play_list = play_list.clone();
        let next_id = next_id.clone();
        {
            let audio = audio.clone();
            decoder.add_event(DecoderEventChannelSink::new(move |e| {
                debug!("解码器事件触发：{e:?}");
                match e.result() {
                    Ok(decoder_event) => match decoder_event {
                        DecoderEventType::Eof => {
                            _ = next_inner(play_list.clone(), true, next_id.clone(), audio.clone());
                        }
                    },
                    Err(err) => {
                        error!("发生解码错误事件： {err:?}")
                    }
                }
            }));
        }
        audio.set_decoder(decoder)?;
        Ok(())
    } else {
        if play_end {
            audio.pause().unwrap_or_else(|e| {
                error!("暂停失败: err: {e}");
            });
            info!("没有下一曲，将暂停")
        } else {
            error!("切换下一曲发生错误, 不存在");
        }
        Ok(())
    }
}

fn create_play_list(
    files: &[PathBuf],
    loop_mode: crate::command::music_play::LoopMode,
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
