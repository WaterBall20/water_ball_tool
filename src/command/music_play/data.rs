use crate::command::music_play::LoopMode;
use std::collections::VecDeque;
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
    next_id: usize,
}
impl NextTrack<'_> {
    pub(super) fn track(&self) -> &Track {
        self.track
    }

    pub(super) fn next_id(&self) -> usize {
        self.next_id
    }
}

pub(super) struct PlayList {
    tracks: Vec<Track>,
    current_index: usize, //tracks的当前索引，对于定期洗牌模式（周期不重复）则为shuffle_bag
    loop_mode: LoopMode,
    // 用于"周期不重复"模式的随机队列
    shuffle_bag: VecDeque<usize>,
    // 换曲ID
    next_id: usize,
}

impl PlayList {
    pub(super) fn new(tracks: Vec<Track>, loop_mode: LoopMode) -> Self {
        let tracks_len = tracks.len();
        Self {
            tracks,
            current_index: tracks_len - 1,
            loop_mode,
            shuffle_bag: VecDeque::with_capacity(tracks_len),
            next_id: 0,
        }
    }

    /// 执行下一首
    /// play_end: 表示播放结束触发
    pub(super) fn next(&mut self, play_end: bool, next_id: usize) -> Option<NextTrack<'_>> {
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
                    if self.shuffle_bag.is_empty() {
                        //用于洗牌的临时列表
                        let mut temp_list = {
                            let mut temp = VecDeque::with_capacity(tracks_len);
                            for i in 0..tracks_len {
                                temp.push_back(i);
                            }
                            temp
                        };
                        while !temp_list.is_empty() {
                            let r = rand::random_range(0..temp_list.len());
                            self.shuffle_bag.push_back(
                                temp_list
                                    .remove(r)
                                    .expect("逻辑错误：用于洗牌的临时列表无法正常获取项"),
                            );
                        }
                    }
                    self.current_index = self.shuffle_bag.pop_front().unwrap_or(0);
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

    pub(super) fn set_loop_mod(&mut self, loop_mode: LoopMode) {
        self.loop_mode = loop_mode;
        if let LoopMode::PeriodicShuffle = self.loop_mode {
            self.shuffle_bag.clear();
        }
    }

    fn update_next_id(&mut self) -> usize {
        if self.next_id == usize::MAX {
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
