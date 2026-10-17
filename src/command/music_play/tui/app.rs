use std::collections::VecDeque;

pub struct App {
    /// 日志缓冲区，限制最大行数避免内存无限增长
    pub logs: VecDeque<String>,
    /// 标记是否应该退出，由事件循环处理
    pub should_quit: bool,
}

impl App {
    pub fn new() -> Self {
        Self {
            logs: VecDeque::with_capacity(1000),
            should_quit: false,
        }
    }

    /// 这是 Update 逻辑的一部分：接收新日志并存入缓冲区
    pub fn push_log(&mut self, line: String) {
        // 保持缓冲区大小，例如最多保留 1000 行
        if self.logs.len() >= 1000 {
            self.logs.pop_front();
        }
        self.logs.push_back(line);
    }
}
