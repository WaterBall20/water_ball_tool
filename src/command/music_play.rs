use std::{error, path::PathBuf};

mod data;
mod running;

use clap::{Args, ValueEnum};
use indicatif::MultiProgress;
#[derive(Args, Debug)]
pub(crate) struct MusicPlayArgs {
    ///播放文件或目录
    #[arg(short, long)]
    files: Vec<PathBuf>,
    ///初始音量 (0到<1的小数或0-100的数，大于或等于1视为0-100)
    #[arg(short, long, default_value_t = 0.75)]
    volume: f32,
    ///详细输出
    #[arg(short, long)]
    verbose: bool,/// 播放模式
    #[arg(short, long, default_value = "sequential-loop")]
    mode: LoopMode,
}

#[derive(ValueEnum, Clone, Debug)]
enum LoopMode {
    SequentialLoop,   // 顺序循环
    Sequential,       // 顺序播放
    RepeatOne,        // 单曲循环
    Shuffle,          // 随机播放
    PeriodicShuffle,  // 周期不重复
}

pub fn args(args: MusicPlayArgs, mp: Option<&MultiProgress>) -> Result<(), Box<dyn error::Error>> {
    let files = args.files;
    let volume = {
        if args.volume < 0.0 {
            panic!("参数错误：声音参数不能小于0")
        } else if args.volume > 100.0 {
            panic!("参数错误：声音参数不能大于100")
        }
    };

    todo!()
}
