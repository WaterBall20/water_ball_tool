use std::{collections::VecDeque, path::PathBuf, time::Duration};

use crate::command::music_play::LoopMode;

pub(super) struct Track {
    path: PathBuf,
    title: String,
    artist: String,
    duration: Duration, // 来自 lofty
}

pub(super) struct PlayList {
    tracks: Vec<Track>,
    current_index: usize,
    loop_mode: LoopMode,
    // 用于"周期不重复"模式的随机队列
    shuffle_bag: VecDeque<usize>,
}
