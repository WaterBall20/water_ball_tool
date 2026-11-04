//! 音乐播放引擎 / Music player engine
//!
//! 只包含与终端 UI 无关的播放引擎：解码线程、cpal 输出、效果链、事件分发、播放列表。
//! 进度条、stdin 命令循环和 TUI 由二进制包负责。
//!
//! Terminal-UI-independent playback engine: decode thread, cpal output, effect chain,
//! event dispatch and playlist. Progress bars, the stdin command loop and the TUI live
//! in the binary crate.

//AI=== 引擎模块树 / engine module tree
pub mod decoder_runtime;
pub mod effect;
pub mod event;
mod event_dispatch;
pub mod play_list;
pub mod player;
pub mod tools;
pub mod watchdog;
//AI_END===

//AI=== 从二进制包移入（去掉 clap 的 ValueEnum 派生）/ moved from the binary crate, clap's ValueEnum derive removed
/// 播放模式 / Playback mode
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoopMode {
    /// 顺序循环 / Sequential loop
    SequentialLoop,
    /// 顺序播放 / Sequential (stop at the end)
    Sequential,
    /// 单曲循环 / Repeat one
    RepeatOne,
    /// 随机播放 / Shuffle
    Shuffle,
    /// 周期不重复 / Periodic shuffle
    PeriodicShuffle,
}
//AI_END===

impl std::str::FromStr for LoopMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "sequential-loop" => Ok(Self::SequentialLoop),
            "sequential" => Ok(Self::Sequential),
            "repeat-one" => Ok(Self::RepeatOne),
            "shuffle" => Ok(Self::Shuffle),
            "periodic-shuffle" => Ok(Self::PeriodicShuffle),
            other => Err(format!(
                "invalid loop mode `{other}` \
                 (expected sequential-loop|sequential|repeat-one|shuffle|periodic-shuffle)"
            )),
        }
    }
}
