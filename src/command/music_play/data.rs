use crate::command::music_play::LoopMode;
use std::fs::File;
use std::path::Path;
use std::{path::PathBuf, time::Duration};

#[derive(Debug)]
pub(super) struct Track {
    path: PathBuf,
    title: String,
    artist: String,
    duration: Duration, // 来自 lofty
}

impl Track {
    pub(super) fn new(path: PathBuf, title: String, artist: String, duration: Duration) -> Self {
        Self {
            path,
            title,
            artist,
            duration,
        }
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }
    pub(super) fn title(&self) -> &str {
        &self.title
    }
    pub(super) fn artist(&self) -> &str {
        &self.artist
    }
    pub(super) fn duration(&self) -> &Duration {
        &self.duration
    }
}

pub(super) struct NextTrack<'t> {
    track: &'t Track,
    next_id: u64,
}
impl NextTrack<'_> {
    pub(super) fn track(&self) -> &Track {
        self.track
    }

    pub(super) fn next_id(&self) -> u64 {
        self.next_id
    }
}

pub(super) struct PlayList {
    tracks: Vec<Track>,
    current_index: usize,
    loop_mode: LoopMode,
    // 用于"周期不重复"模式的随机队列
    shuffle_bag: Vec<usize>,
    // 换曲ID
    next_id: u64,
}

impl PlayList {
    pub(super) fn new(tracks: Vec<Track>, loop_mode: LoopMode) -> Self {
        let tracks_len = tracks.len();
        Self {
            tracks,
            current_index: tracks_len - 1,
            loop_mode,
            shuffle_bag: Vec::with_capacity(tracks_len),
            next_id: 0,
        }
    }

    /// 执行下一首
    /// play_end: 表示播放结束触发
    pub(super) fn next(&mut self, play_end: bool, next_id: u64) -> Option<NextTrack<'_>> {
        if self.tracks.is_empty() {
            return None;
        }
        if !play_end || self.next_id == next_id {
            let next_id = self.update_next_id();
            match &self.loop_mode {
                LoopMode::SequentialLoop => {
                    // 顺序循环
                    let tracks_len = self.tracks.len();
                    let index = self.current_index + 1;
                    self.current_index = if index >= tracks_len { 0 } else { index };
                }
                LoopMode::Sequential => {
                    // 顺序播放，仅手动时从头播放
                    let tracks_len = self.tracks.len();
                    let index = self.current_index + 1;
                    if index >= tracks_len {
                        if play_end {
                            return None;
                        }
                        self.current_index = 0;
                    } else {
                        self.current_index = index;
                    }
                }
                LoopMode::RepeatOne => {
                    //单曲循环，仅手动下一曲
                    if !play_end {
                        let tracks_len = self.tracks.len();
                        let index = self.current_index + 1;
                        self.current_index = if index >= tracks_len { 0 } else { index };
                    }
                }
                LoopMode::Shuffle => {
                    // 随机播放
                    let tracks_len = self.tracks.len();
                    self.current_index = rand::random_range(0..tracks_len);
                }
                LoopMode::PeriodicShuffle => {
                    // 周期不循环
                    let tracks_len = self.tracks.len();
                    // 重置
                    if self.shuffle_bag.len() >= tracks_len {
                        self.shuffle_bag.clear();
                    }
                    'root: loop {
                        let random_index = rand::random_range(0..tracks_len);
                        // 重复性检查
                        for i in &self.shuffle_bag {
                            if *i == random_index {
                                continue 'root;
                            }
                        }
                        self.shuffle_bag.push(random_index);
                        self.current_index = random_index;
                        break;
                    }
                }
            };
            Some(NextTrack {
                track: self.get_current_track().expect("没有音轨"),
                next_id,
            })
        } else {
            None
        }
    }

    fn update_next_id(&mut self) -> u64 {
        if self.next_id == u64::MAX {
            self.next_id = 0;
        } else {
            self.next_id += 1;
        }
        self.next_id
    }

    pub(super) fn get_current_track(&self) -> Option<&Track> {
        self.tracks.get(self.current_index)
    }

    pub(super) fn loop_mod(&self) -> LoopMode {
        self.loop_mode.clone()
    }

    pub(super) fn reset(&mut self) {
        self.current_index = 0;
        self.shuffle_bag.clear();
    }
}
