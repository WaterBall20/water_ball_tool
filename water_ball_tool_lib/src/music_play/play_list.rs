use crate::music_play::LoopMode;
use lofty::file::FileType;
use std::collections::VecDeque;
use std::path::Path;
use std::{path::PathBuf, time::Duration};
//AI=== 引入 tracing 以记录错误（替代 expect 的 panic）/ Import tracing to log errors instead of panicking via expect
use tracing::error;
//AI_END===

#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    path: PathBuf,
    title: String,
    artist: String,
    duration: Duration, // 来自 lofty
    file_type: FileType,
}

impl Track {
    #[must_use]
    pub fn new(
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

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }
    #[must_use]
    pub fn artist(&self) -> &str {
        &self.artist
    }
    #[must_use]
    pub fn duration(&self) -> &Duration {
        &self.duration
    }
}

pub struct NextTrack<'t> {
    track: &'t Track,
    next_id: usize,
}
impl NextTrack<'_> {
    #[must_use]
    pub fn track(&self) -> &Track {
        self.track
    }

    #[must_use]
    pub fn next_id(&self) -> usize {
        self.next_id
    }
}

//AI=== 播放轨迹容量上限（构造时预分配，长度从 0 增长到上限）
// Upper bound of the navigation history; allocated up front, length grows from 0.
const HISTORY_CAP: usize = 1024;
//AI_END===

pub struct PlayList {
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
    //AI=== 播放轨迹：next 离开当前曲时压入其索引，prev 弹出；只由 next 写入
    // Navigation history: `next` pushes the index it leaves, `prev` pops it. Only `next` writes.
    history: VecDeque<usize>,
    //AI_END===
}

impl PlayList {
    #[must_use]
    pub fn new(tracks: Vec<Track>, loop_mode: LoopMode) -> Self {
        let tracks_len = tracks.len();
        Self {
            tracks,
            current_index: 0,
            loop_mode,
            rand_list: VecDeque::with_capacity(tracks_len),
            next_id: 0,
            //AI=== 轨迹预分配容量、初始为空 / history is pre-allocated with capacity, starting empty
            history: VecDeque::with_capacity(HISTORY_CAP),
            //AI_END===
        }
    }

    /// 执行下一首
    /// play_end: 表示播放结束触发
    pub fn next(&mut self, play_end: bool, next_id: usize) -> Option<NextTrack<'_>> {
        if self.tracks.is_empty() {
            return None;
        }
        if !play_end || self.next_id == next_id {
            let next_id = self.update_next_id();
            //AI=== 记录换曲前的索引，用于压入播放轨迹 / remember the outgoing index for the history
            let prev_index = self.current_index;
            //AI_END===
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
                    //AI=== 索引从 1 开始（`get_current_track` 取 `current_index - 1`），0 会下溢且永远播不到末曲
                    // indices are 1-based (`get_current_track` uses `current_index - 1`); 0 underflows and the last track would never play
                    let tracks_len = self.tracks.len();
                    self.current_index = rand::random_range(1..=tracks_len);
                    //AI_END===
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
                            if let Some(index) = temp_list.remove(r) {
                                self.rand_list.push_back(index)
                            } else {
                                error!("shuffle temp list remove failed / 洗牌临时列表移除失败");
                                break;
                            }
                            //AI_END===
                        }
                    }
                    //AI=== 周期边界：新周期的首曲不能是当前正在播的曲目，否则跨周期会连续重复
                    // Cycle boundary: the fresh cycle must not start with the track that is still playing,
                    // otherwise it repeats back to back across the boundary.
                    avoid_immediate_repeat(&mut self.rand_list, self.current_index);
                    //AI_END===
                    self.current_index = self.rand_list.pop_front().unwrap_or(0);
                }
            }
            //AI=== 只有真正换了曲目才记录轨迹（单曲循环的自动循环原地不动，不记录）
            // Only record when the track actually changed (RepeatOne's automatic loop stays put).
            if prev_index != 0 && self.current_index != prev_index {
                self.history.push_back(prev_index);
                if self.history.len() > HISTORY_CAP {
                    self.history.pop_front();
                }
            }
            //AI_END===
            //AI=== 没有当前音轨时记录 error! 并返回 None，不 panic / Log error! and return None when there is no current track instead of panicking
            if let Some(track) = self.get_current_track() {
                Some(NextTrack { track, next_id })
            } else {
                error!("no current track / 没有当前音轨");
                None
            }
            //AI_END===
        } else {
            None
        }
    }

    pub fn set_loop_mod(&mut self, loop_mode: LoopMode) {
        self.loop_mode = loop_mode;
        if let LoopMode::PeriodicShuffle = self.loop_mode {
            self.rand_list.clear();
        }
    }

    //AI=== 上一曲：沿播放轨迹回退一格 / previous track: step back along the navigation history
    pub fn prev(&mut self) -> Option<NextTrack<'_>> {
        // 轨迹只由 next 写入，这里只弹出；退到轨迹起点后返回 None
        // The history is written by next only; prev just pops. None once the start of the history is reached.
        let index = self.history.pop_back()?;
        self.current_index = index;
        // 周期洗牌：落点若还在本周期待播队列中，移出，避免本周期内重复播放
        // Periodic shuffle: drop the landing track from the current cycle queue so it cannot repeat within the cycle.
        if let LoopMode::PeriodicShuffle = self.loop_mode
            && let Some(pos) = self.rand_list.iter().position(|&i| i == index)
        {
            _ = self.rand_list.remove(pos);
        }
        let next_id = self.update_next_id();
        if let Some(track) = self.get_current_track() {
            Some(NextTrack { track, next_id })
        } else {
            error!("no current track / 没有当前音轨");
            None
        }
    }
    //AI_END===

    fn update_next_id(&mut self) -> usize {
        if self.next_id == usize::MAX {
            self.next_id = 0;
        } else {
            self.next_id += 1;
        }
        self.next_id
    }

    #[must_use]
    pub fn get_current_track(&self) -> Option<&Track> {
        self.tracks.get(self.current_index - 1)
    }

    //AI=== 追加 / 插入：单曲与批量各一对，批量是唯一实现 / append & insert: one pair each for a single track and a batch; the batch one is the only implementation
    /// 追加单曲 / Append a single track.
    pub fn add_track(&mut self, track: Track) {
        self.insert_tracks(self.tracks.len() + 1, vec![track]);
    }

    /// 追加多曲 / Append several tracks.
    pub fn add_tracks(&mut self, tracks: Vec<Track>) {
        self.insert_tracks(self.tracks.len() + 1, tracks);
    }

    /// 在 1-based 位置 pos 插入单曲 / Insert a single track at the 1-based position `pos`.
    pub fn insert_track(&mut self, pos: usize, track: Track) {
        self.insert_tracks(pos, vec![track]);
    }

    /// 在 1-based 位置 pos 插入多曲；pos 会夹到 `1..=len+1`
    /// Insert several tracks at the 1-based position `pos`, clamped to `1..=len+1`.
    pub fn insert_tracks(&mut self, pos: usize, tracks: Vec<Track>) {
        let count = tracks.len();
        if count == 0 {
            return;
        }
        let tracks_len = self.tracks.len();
        let pos = pos.clamp(1, tracks_len + 1);
        self.tracks.splice(pos - 1..pos - 1, tracks);

        // 只有中间插入才需要后移既有索引；追加到末尾不涉及任何既有项
        // Only a middle insertion shifts existing indices; appending at the end touches nothing.
        if pos <= tracks_len {
            if self.current_index >= pos {
                self.current_index += count;
            }
            for index in &mut self.history {
                if *index >= pos {
                    *index += count;
                }
            }
            for index in &mut self.rand_list {
                if *index >= pos {
                    *index += count;
                }
            }
        }

        // 周期洗牌：把新曲补进本周期待播队列，否则要等下一周期才轮得到
        // Periodic shuffle: queue the new tracks for the current cycle instead of waiting for the next one.
        if let LoopMode::PeriodicShuffle = self.loop_mode
            && !self.rand_list.is_empty()
        {
            self.rand_list.extend(pos..pos + count);
        }
    }
    //AI_END===

    #[must_use]
    pub fn loop_mod(&self) -> LoopMode {
        self.loop_mode
    }

    pub fn reset(&mut self) {
        self.current_index = 0;
        self.rand_list.clear();
        //AI=== 轨迹一并清空 / clear the navigation history as well
        self.history.clear();
        //AI_END===
    }
}

//AI=== 周期边界处理：新周期首曲排除当前播放索引 / cycle boundary: exclude the currently playing index from the start of a new cycle
fn avoid_immediate_repeat(rand_list: &mut VecDeque<usize>, current_index: usize) {
    if rand_list.len() > 1 && rand_list.front() == Some(&current_index) {
        let other = rand::random_range(1..rand_list.len());
        rand_list.swap(0, other);
    }
}
//AI_END===

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

    //AI=== 测试辅助：独立曲目 / 推进一首 / 回退一首 / helpers: standalone track, advance one, step back one
    fn track(name: &str) -> Track {
        Track::new(
            PathBuf::from(name),
            name.to_string(),
            name.to_string(),
            Duration::default(),
            FileType::Aac,
        )
    }

    fn advance(play_list: &mut PlayList, play_end: bool, next_id: &mut usize) -> Track {
        match play_list.next(play_end, *next_id) {
            Some(r) => {
                *next_id = r.next_id;
                r.track.clone()
            }
            None => panic!("next failed"),
        }
    }

    fn prev_track(play_list: &mut PlayList) -> Option<Track> {
        play_list.prev().map(|r| r.track.clone())
    }
    //AI_END===

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
        //AI=== 记录上一周期末曲，用于检查跨周期边界 / keep the previous cycle's last track to check the boundary
        let mut prev_cycle_last: Option<Track> = None;
        //AI_END===
        for _ in 0..len {
            for _ in 0..len {
                let r = play_list.next(false, next_id).expect("next failed");
                next_id = r.next_id;
                for v in &next_list {
                    assert_ne!(v, r.track);
                }
                //AI=== 新周期首曲不得与上一周期末曲相同 / a new cycle must not start with the previous cycle's last track
                if next_list.is_empty()
                    && let Some(prev) = &prev_cycle_last
                {
                    assert_ne!(prev, r.track);
                }
                //AI_END===
                next_list.push(r.track.clone());
            }
            //AI=== 保存末曲后清空 / store the last track before clearing
            prev_cycle_last = next_list.last().cloned();
            //AI_END===
            next_list.clear();
        }
    }

    //AI=== 边界单元测试：首项等于当前曲时必须换位且不丢项 / boundary unit tests: swap when the front equals the current track, losing nothing
    #[test]
    fn cycle_boundary_moves_current_out_of_front() {
        let mut rand_list = VecDeque::from([3, 1, 2]);
        avoid_immediate_repeat(&mut rand_list, 3);
        assert_ne!(rand_list.front(), Some(&3));
        let mut sorted: Vec<usize> = rand_list.iter().copied().collect();
        sorted.sort_unstable();
        assert_eq!(sorted, vec![1, 2, 3]);
    }

    #[test]
    fn cycle_boundary_keeps_queue_when_front_differs() {
        let mut rand_list = VecDeque::from([1, 2, 3]);
        avoid_immediate_repeat(&mut rand_list, 3);
        assert_eq!(rand_list, VecDeque::from([1, 2, 3]));
    }

    #[test]
    fn cycle_boundary_single_track_unchanged() {
        let mut rand_list = VecDeque::from([1]);
        avoid_immediate_repeat(&mut rand_list, 1);
        assert_eq!(rand_list, VecDeque::from([1]));
    }
    //AI_END===

    //AI=== 洗牌索引必须落在 1..=len，单曲列表也要能播 / shuffle indices must stay within 1..=len; a single-track list must play
    #[test]
    fn shuffle_single_track_plays() {
        let list = creat_tack_list(1);
        let mut play_list = PlayList::new(list.clone(), LoopMode::Shuffle);
        let Some(r) = play_list.next(false, 0) else {
            panic!("next failed");
        };
        assert_eq!(r.track, &list[0]);
    }
    //AI_END===

    //AI=== 播放轨迹与上一曲 / navigation history and previous track
    #[test]
    fn history_records_leaving_track() {
        let mut play_list = PlayList::new(creat_tack_list(5), LoopMode::SequentialLoop);
        let mut id = 0;
        for _ in 0..3 {
            advance(&mut play_list, false, &mut id);
        }
        // 播到第 3 首：轨迹里是离开过的第 1、2 首
        assert_eq!(VecDeque::from([1, 2]), play_list.history);
    }

    #[test]
    fn prev_walks_back_to_history_start() {
        let list = creat_tack_list(5);
        let mut play_list = PlayList::new(list.clone(), LoopMode::SequentialLoop);
        let mut id = 0;
        for _ in 0..3 {
            advance(&mut play_list, false, &mut id);
        }
        assert_eq!(Some(list[1].clone()), prev_track(&mut play_list));
        assert_eq!(Some(list[0].clone()), prev_track(&mut play_list));
        assert_eq!(None, prev_track(&mut play_list));
        assert_eq!(1, play_list.current_index);
    }

    #[test]
    fn repeat_one_auto_loop_does_not_record() {
        let list = creat_tack_list(3);
        let mut play_list = PlayList::new(list.clone(), LoopMode::RepeatOne);
        let mut id = 0;
        advance(&mut play_list, false, &mut id);
        advance(&mut play_list, false, &mut id); // 第 2 首，轨迹 [1]
        let auto = advance(&mut play_list, true, &mut id); // 自动循环原地不动
        assert_eq!(list[1], auto);
        assert_eq!(VecDeque::from([1]), play_list.history);
    }

    #[test]
    fn history_is_bounded_by_cap() {
        let total = HISTORY_CAP + 76;
        let mut play_list = PlayList::new(creat_tack_list(total), LoopMode::SequentialLoop);
        let mut id = 0;
        for _ in 0..total {
            advance(&mut play_list, false, &mut id);
        }
        assert_eq!(HISTORY_CAP, play_list.history.len());
        // 最旧的 76 条被丢弃，栈顶是最后离开的那首
        assert_eq!(Some(&(total - 1)), play_list.history.back());
        let mut steps = 0;
        while prev_track(&mut play_list).is_some() {
            steps += 1;
            assert!(steps <= HISTORY_CAP);
        }
        assert_eq!(HISTORY_CAP, steps);
    }
    //AI_END===

    //AI=== 追加 / 插入 / append & insert
    #[test]
    fn add_track_appends_without_touching_history() {
        let mut play_list = PlayList::new(creat_tack_list(5), LoopMode::SequentialLoop);
        let mut id = 0;
        for _ in 0..3 {
            advance(&mut play_list, false, &mut id);
        }
        play_list.add_track(track("extra"));
        assert_eq!(6, play_list.tracks.len());
        assert_eq!(3, play_list.current_index);
        assert_eq!(VecDeque::from([1, 2]), play_list.history);
        assert_eq!(Some(&track("extra")), play_list.tracks.get(5));
    }

    #[test]
    fn add_tracks_appends_without_touching_history() {
        let mut play_list = PlayList::new(creat_tack_list(5), LoopMode::SequentialLoop);
        let mut id = 0;
        for _ in 0..3 {
            advance(&mut play_list, false, &mut id);
        }
        play_list.add_tracks(vec![track("x"), track("y")]);
        assert_eq!(7, play_list.tracks.len());
        assert_eq!(3, play_list.current_index);
        assert_eq!(VecDeque::from([1, 2]), play_list.history);
    }

    #[test]
    fn insert_track_in_middle_updates_history() {
        let list = creat_tack_list(5);
        let mut play_list = PlayList::new(list.clone(), LoopMode::SequentialLoop);
        let mut id = 0;
        for _ in 0..5 {
            advance(&mut play_list, false, &mut id);
        } // 当前第 5 首，轨迹 [1,2,3,4]
        play_list.insert_track(2, track("extra"));
        assert_eq!(6, play_list.tracks.len());
        // 当前曲后移到第 6 位，轨迹里 >= 2 的项整体 +1
        assert_eq!(6, play_list.current_index);
        assert_eq!(Some(&list[4]), play_list.get_current_track());
        assert_eq!(VecDeque::from([1, 3, 4, 5]), play_list.history);
        // 上一曲仍指向原来第 4 首
        assert_eq!(Some(list[3].clone()), prev_track(&mut play_list));
    }

    #[test]
    fn insert_tracks_at_end_keeps_history() {
        let mut play_list = PlayList::new(creat_tack_list(5), LoopMode::SequentialLoop);
        let mut id = 0;
        for _ in 0..5 {
            advance(&mut play_list, false, &mut id);
        }
        play_list.insert_tracks(6, vec![track("x"), track("y")]);
        assert_eq!(7, play_list.tracks.len());
        assert_eq!(5, play_list.current_index);
        assert_eq!(VecDeque::from([1, 2, 3, 4]), play_list.history);
    }

    #[test]
    fn insert_keeps_periodic_cycle_indices_valid() {
        let mut play_list = PlayList::new(creat_tack_list(10), LoopMode::PeriodicShuffle);
        let mut id = 0;
        for _ in 0..3 {
            advance(&mut play_list, false, &mut id);
        }
        let before = play_list.tracks.len();
        play_list.insert_track(1, track("extra"));
        assert_eq!(before + 1, play_list.tracks.len());
        // 待播队列里的索引仍然有效、不重复，新曲已进入本周期，当前曲不在队列中
        assert!(play_list.rand_list.contains(&1));
        let mut seen = std::collections::HashSet::new();
        for &index in &play_list.rand_list {
            assert!((1..=play_list.tracks.len()).contains(&index));
            assert!(seen.insert(index));
        }
        assert!(!seen.contains(&play_list.current_index));
    }
    //AI_END===
}
