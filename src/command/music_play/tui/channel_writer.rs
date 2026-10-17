use std::io::{self, Write};
use std::sync::mpsc;

/// 一个将写入内容转换为 String 并通过 mpsc 发送的 Writer。
struct ChannelWriter {
    tx: mpsc::Sender<String>,
    buffer: Vec<u8>,
}

impl Write for ChannelWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buffer.extend_from_slice(buf);
        // 遇到换行符，认为是一条完整的日志
        if let Some(pos) = self.buffer.iter().position(|&b| b == b'\n') {
            let line = String::from_utf8_lossy(&self.buffer[..pos]).into_owned();
            // 忽略发送错误（例如接收端已关闭），避免日志系统本身崩溃
            let _ = self.tx.send(line);
            self.buffer.clear();
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Clone)]
struct ChannelMakeWriter(mpsc::Sender<String>);

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for ChannelMakeWriter {
    type Writer = ChannelWriter;

    fn make_writer(&'a self) -> Self::Writer {
        ChannelWriter {
            tx: self.0.clone(),
            buffer: Vec::with_capacity(256),
        }
    }
}

/// 初始化日志系统，返回日志接收端。
pub fn init_logging() -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(ChannelMakeWriter(tx))
        .with_target(false) // 可以按需开启
        .init();

    rx
}
