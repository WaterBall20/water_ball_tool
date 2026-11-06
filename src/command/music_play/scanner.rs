use lofty::file::{AudioFile, FileType, TaggedFile, TaggedFileExt};
use lofty::prelude::Accessor;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::{error, io, thread};
use tracing::{error, warn};
use water_ball_tool_lib::file_finder::{FileFinder, FileKind, SearchEvent};
use water_ball_tool_lib::music_play::play_list::Track;

// 扫描→UI 的队列：曲目必须送达，进度只是尽力而为
// Scanner → UI queue: tracks must arrive; progress is best-effort.
const SCAN_CHANNEL_CAP: usize = 256;

pub(super) enum ScanMessage {
    Track(Track),
    Progress { files: u64, dirs: u64 },
    Done,
}

/// 启动扫描线程，返回曲目/进度接收端 / spawn the scanner thread and return the track/progress receiver
pub(super) fn spawn(paths: Vec<PathBuf>) -> io::Result<Receiver<ScanMessage>> {
    let (tx, rx) = mpsc::sync_channel(SCAN_CHANNEL_CAP);
    thread::Builder::new()
        .name("mp-scan".to_string())
        .spawn(move || scan(paths, tx))?;
    Ok(rx)
}

fn scan(paths: Vec<PathBuf>, tx: SyncSender<ScanMessage>) {
    let mut counts = (0u64, 0u64);
    for path in paths {
        if path.is_file() {
            if !send_track(&tx, &path) {
                return;
            }
        } else if path.is_dir() {
            counts = scan_dir(&tx, &path, counts);
        } else {
            warn!(
                "路径不存在或不可访问 / path missing or inaccessible: {}",
                path.display()
            );
        }
    }
    let _ = tx.send(ScanMessage::Done);
}

/// 扫描一个目录，返回累加后的（文件数，目录数）/ scan one directory and return the cumulative (files, dirs) counts
fn scan_dir(tx: &SyncSender<ScanMessage>, dir: &Path, base: (u64, u64)) -> (u64, u64) {
    let finder = FileFinder::new();
    let (progress_tx, progress_rx) = mpsc::channel();
    let (handle, rx) = match finder.search_stream(dir, true, progress_tx, 4) {
        Ok(result) => result,
        Err(e) => {
            error!("创建搜索器失败 / failed to create finder: {e}");
            return base;
        }
    };

    let mut counts = base;
    while let Ok(event) = rx.recv() {
        match event {
            SearchEvent::Entry(path, info) => {
                // 目录遍历只接受 lofty 能识别的扩展名；显式传入的文件不走此过滤
                // Directory walks only accept extensions lofty recognizes; explicitly passed files bypass this filter.
                if matches!(info.file_kind(), FileKind::File { .. })
                    && FileType::from_path(&path).is_some()
                    && !send_track(tx, &path)
                {
                    return counts;
                }
            }
            SearchEvent::Warning(w) => warn!("搜索警告 / search warning: {w:?}"),
        }

        drain_progress(tx, &progress_rx, &mut counts);
    }

    drain_progress(tx, &progress_rx, &mut counts);
    if let Err(e) = handle.join() {
        error!("搜索线程异常退出 / search thread failed: {e:?}");
    }
    counts
}

fn drain_progress(
    tx: &SyncSender<ScanMessage>,
    progress_rx: &Receiver<(u64, u64)>,
    counts: &mut (u64, u64),
) {
    while let Ok((add_files, add_dirs)) = progress_rx.try_recv() {
        counts.0 += add_files;
        counts.1 += add_dirs;
        // 进度只做尽力投递，不阻塞扫描线程
        // Progress is best-effort and never blocks the scanner.
        let _ = tx.try_send(ScanMessage::Progress {
            files: counts.0,
            dirs: counts.1,
        });
    }
}

/// 发送一首曲目；读取失败只记录并继续 / send one track; a read failure is logged and skipped
fn send_track(tx: &SyncSender<ScanMessage>, path: &Path) -> bool {
    match read_track(path) {
        Ok(track) => tx.send(ScanMessage::Track(track)).is_ok(),
        Err(e) => {
            warn!(
                "读取音频信息失败 / failed to read tags: {}: {e}",
                path.display()
            );
            true
        }
    }
}

fn read_track(path: &Path) -> Result<Track, Box<dyn error::Error>> {
    let tagged = lofty::read_from_path(path)?;
    let file_type = tagged.file_type();
    let duration = tagged.properties().duration();

    let (title, artist, album) = match tagged.primary_tag() {
        Some(tag) => (
            tag.title()
                .as_deref()
                .map_or_else(|| fallback_title(path), str::to_string),
            tag.artist().as_deref().unwrap_or("Unknown").to_string(),
            tag.album().as_deref().map(str::to_string),
        ),
        None => (fallback_title(path), "Unknown".to_string(), None),
    };

    let mut track = Track::new(path.to_path_buf(), title, artist, duration, file_type);
    track.set_album(album);
    track.set_audio_info(format_audio_info(&tagged));
    Ok(track)
}

fn fallback_title(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Unknown")
        .to_string()
}

fn format_audio_info(tagged: &TaggedFile) -> String {
    let props = tagged.properties();
    let mut parts = Vec::new();
    if let Some(rate) = props.sample_rate() {
        parts.push(format!("{:.1} kHz", f64::from(rate) / 1000.0));
    }
    if let Some(depth) = props.bit_depth() {
        parts.push(format!("{depth} bit"));
    }
    if let Some(channels) = props.channels() {
        parts.push(format!("{channels} ch"));
    }
    if let Some(bitrate) = props.audio_bitrate() {
        parts.push(format!("{} kbps", bitrate / 1000));
    }

    let file_type = format!("{:?}", tagged.file_type());
    if parts.is_empty() {
        file_type
    } else {
        format!("{file_type} · {}", parts.join(" · "))
    }
}
