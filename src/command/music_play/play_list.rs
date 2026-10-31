use crate::command::music_play::LoopMode;
use lofty::file::FileType;
use std::collections::VecDeque;
use std::path::Path;
use std::{path::PathBuf, time::Duration};
//AI=== 引入 tracing 以记录错误（替代 expect 的 panic）/ Import tracing to log errors instead of panicking via expect
use tracing::error;
//AI_END===

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Track {
    path: PathBuf,
    title: String,
    artist: String,
    duration: Duration, // 来自 lofty
    file_type: FileType,
}

impl Track {
    pub(super) fn new(
        path: PathBuf,
        title: String,
        artist: String,
        duration: Duration,
        file_type: FileType,
    ) -> Self {
        Self {
            path,
            title,
            artist,
            duration,
            file_type,
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
    /// tracks的当前索引，对于定期洗牌模式（周期不重复）不使用。
    /// # 注意
    /// 索引的有效值从1开始
    /// 0表示初始状态
    current_index: usize,
    loop_mode: LoopMode,
    // 用于随机模式的随机队列
    rand_list: VecDeque<usize>,
    // 换曲ID
    next_id: usize,
}

impl PlayList {
    pub(super) fn new(tracks: Vec<Track>, loop_mode: LoopMode) -> Self {
        let tracks_len = tracks.len();
        Self {
            tracks,
            current_index: 0,
            loop_mode,
            rand_list: VecDeque::with_capacity(tracks_len),
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
                    self.current_index = if index > tracks_len { 1 } else { index };
                }
                LoopMode::Sequential => {
                    // 顺序播放，仅手动时从头播放
                    let tracks_len = self.tracks.len();
                    let index = self.current_index + 1;
                    if index > tracks_len {
                        if play_end {
                            return None;
                        }
                        self.current_index = 1;
                    } else {
                        self.current_index = index;
                    }
                }
                LoopMode::RepeatOne => {
                    //单曲循环，仅手动下一曲
                    if !play_end {
                        let tracks_len = self.tracks.len();
                        let index = self.current_index + 1;
                        self.current_index = if index > tracks_len { 1 } else { index };
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
                    if self.rand_list.is_empty() {
                        //用于洗牌的临时列表
                        let mut temp_list = {
                            let mut temp = VecDeque::with_capacity(tracks_len);
                            for i in 1..=tracks_len {
                                temp.push_back(i);
                            }
                            temp
                        };
                        while !temp_list.is_empty() {
                            let r = rand::random_range(0..temp_list.len());
                            //AI=== 移除 expect：取不到项时记录 error! 并结束洗牌，避免 panic / Remove expect: log error! and stop shuffling when the item is missing, avoiding a panic
                            match temp_list.remove(r) {
                                Some(index) => self.rand_list.push_back(index),
                                None => {
                                    error!(
                                        "shuffle temp list remove failed / 洗牌临时列表移除失败"
                                    );
                                    break;
                                }
                            }
                            //AI_END===
                        }
                    }
                    self.current_index = self.rand_list.pop_front().unwrap_or(0);
                }
            }
            //AI=== 没有当前音轨时记录 error! 并返回 None，不 panic / Log error! and return None when there is no current track instead of panicking
            match self.get_current_track() {
                Some(track) => Some(NextTrack { track, next_id }),
                None => {
                    error!("no current track / 没有当前音轨");
                    None
                }
            }
            //AI_END===
        } else {
            None
        }
    }

    pub(super) fn set_loop_mod(&mut self, loop_mode: LoopMode) {
        self.loop_mode = loop_mode;
        if let LoopMode::PeriodicShuffle = self.loop_mode {
            self.rand_list.clear();
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
        self.tracks.get(self.current_index - 1)
    }

    pub(super) fn loop_mod(&self) -> LoopMode {
        self.loop_mode.clone()
    }

    pub(super) fn reset(&mut self) {
        self.current_index = 0;
        self.rand_list.clear();
    }
}

#[cfg(test)]
mod test {
    use super::*;

    const TEST_COUNT: usize = 100_000;
    const TEST_FOR_COUNT: usize = 100;
    fn creat_tack_list(len: usize) -> Vec<Track> {
        let mut list = Vec::with_capacity(len);
        for i in 0..len {
            let i_str = i.to_string();
            list.push(Track::new(
                PathBuf::from(&i_str),
                i_str.clone(),
                i_str.clone(),
                Duration::default(),
                FileType::Aac,
            ));
        }
        list
    }

    #[test]
    fn sequential_loop_list_no_play_end() {
        sequential_loop_list(false);
    }

    #[test]
    fn sequential_loop_list_play_end() {
        sequential_loop_list(true);
    }
    fn sequential_loop_list(play_end: bool) {
        let list = creat_tack_list(TEST_COUNT);
        let mut next_id = 0;
        let mut play_list = PlayList::new(list.clone(), LoopMode::SequentialLoop);
        for _ in 0..TEST_FOR_COUNT {
            for item in &list {
                assert_eq!(item, {
                    let r = play_list.next(play_end, next_id).expect("next failed");
                    next_id = r.next_id;
                    r.track
                });
            }
        }
    }

    #[test]
    fn sequential_list_play_end() {
        let list = creat_tack_list(TEST_COUNT);
        let mut next_id = 0;
        let mut play_list = PlayList::new(list.clone(), LoopMode::Sequential);
        for item in &list {
            assert_eq!(item, {
                let r = play_list.next(true, next_id).expect("next failed");
                next_id = r.next_id;
                r.track
            });
        }
        assert!(play_list.next(true, next_id).is_none());
    }
    #[test]
    fn sequential_list_no_play_end() {
        let list = creat_tack_list(TEST_COUNT);
        let mut next_id = 0;
        let mut play_list = PlayList::new(list.clone(), LoopMode::Sequential);
        for _ in 0..TEST_FOR_COUNT {
            for item in &list {
                assert_eq!(item, {
                    let r = play_list.next(false, next_id).expect("next failed");
                    next_id = r.next_id;
                    r.track
                });
            }
        }
    }

    #[test]
    fn periodic_shuffle() {
        let len = 10;
        let list = creat_tack_list(len);
        let mut next_id = 0;
        let mut play_list = PlayList::new(list, LoopMode::PeriodicShuffle);
        let mut next_list = Vec::with_capacity(len);
        for _ in 0..len {
            for _ in 0..len {
                let r = play_list.next(false, next_id).expect("next failed");
                next_id = r.next_id;
                for v in &next_list {
                    assert_ne!(v, r.track);
                }
                next_list.push(r.track.clone());
            }
            next_list.clear();
        }
    }
}
