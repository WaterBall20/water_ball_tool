use indicatif::MultiProgress;
use std::io::Write;
use std::{env, io};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, fmt};
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
            _ => {}
        }
    }
}

// 创建一个包装类，让 MultiProgress 兼容 io::Write
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

//初始化
fn init_global_logging(mp: &MultiProgress) {
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
