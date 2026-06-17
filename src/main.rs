use indicatif::MultiProgress;
use std::io::Write;
use std::{env, io};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter};
// 开始时间:2026-02-08 22:37

mod command;

fn main() {
    let mp = MultiProgress::new();
    init_global_logging(&mp);
    //命令行参数处理
    let args: Vec<String> = env::args().collect();
    if let Some(mod_type) = args.get(1) {
        let args = &args[2..];
        match &mod_type[..] {
            "ff" => command::ff(args, Some(&mp)),
            "wbfp" => command::wbfp(args, Some(&mp)),
            v => panic!("未知命令： {v}"),
        }
    } else {
        //TODO:优化提示
        panic!("未提供命令,可用命令:ff、wbfp")
    }
}

// 创建一个包装类，让 MultiProgress 兼容 pack_io::Write
struct MultiProgressWriter(MultiProgress);

impl Write for MultiProgressWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let s = String::from_utf8_lossy(buf);
        // 去掉末尾换行符，因为 mp.println 会自动加
        self.0.println(s.trim_end())?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// 初始化全局日志系统，将 tracing 输出重定向到 indicatif 进度条。
///
/// 在 debug 模式下日志级别为 `debug`，release 模式下为 `info`。
/// 日志通过 `MultiProgressWriter` 适配器输出，与进度条共存而不互相干扰。
///
/// Initialize global logging, redirecting tracing output to the indicatif progress bar.
///
/// Log level is `debug` in debug mode and `info` in release mode.
/// Logs are routed through a `MultiProgressWriter` adapter to coexist with the progress bar.
//AI===
pub fn init_global_logging(mp: &MultiProgress) {
    #[cfg(debug_assertions)]
    let filter = EnvFilter::new("debug"); // 开发模式看 debug
    #[cfg(not(debug_assertions))]
    let filter = EnvFilter::new("info"); // 发布模式只看 warn/error

    // 将 MultiProgress 包装成一个可克隆的 Writer 工厂
    let mp_writer = mp.clone();

    _ = tracing_subscriber::registry()
        .with(filter)
        .with(
            fmt::layer()
                .with_target(false)
                // 关键点：将日志重定向到 MultiProgress 的 println
                .with_writer(move || MultiProgressWriter(mp_writer.clone())),
        )
        .try_init();
}
//AI_END===
