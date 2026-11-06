use super::channel_writer::LogLine;
use crate::command::music_play::runtime::{DebugInfo, Snapshot};
//AI=== 播放列表滚动与鼠标命中需要 Rect/Position / playlist scrolling and mouse hit-testing need Rect/Position
use ratatui::layout::{Position, Rect};
//AI_END===
use std::collections::VecDeque;
//AI=== 左右方向键 seek 手势需要时间戳 / left/right seek gestures need timestamps
use std::time::{Duration, Instant};
//AI_END===
use water_ball_tool_lib::music_play::LoopMode;

//AI=== TUI 状态：播放字段由 Runtime 快照填充 / TUI state, playback fields filled from a Runtime snapshot
pub struct App {
    /// 日志缓冲区，限制最大行数避免内存无限增长 / bounded log buffer
    pub logs: VecDeque<LogLine>,
    /// 标记是否应该退出，由事件循环处理 / quit flag handled by the event loop
    pub should_quit: bool,
    /// 当前曲目信息 / current track info
    pub title: String,
    pub artist: String,
    pub album: String,
    pub audio_info: String,
    pub path: String,
    /// 播放位置与总时长 / playback position and total duration
    pub pos: Duration,
    pub duration: Duration,
    /// 音量（0.0–1.0）/ volume as a 0.0–1.0 fraction
    pub volume: f32,
    /// 循环模式显示名 / loop mode display name
    pub mode: String,
    pub playing: bool,
    /// 解码器毒化（显示异常暂停）/ decoder poisoned (shows the error state)
    pub poisoned: bool,
    /// 播放列表显示行与当前项 / playlist display lines and current index
    pub playlist: Vec<String>,
    pub current: Option<usize>,
    /// 播放列表视口：首行偏移、可见行数与鼠标命中区域
    /// Playlist viewport: first visible row, visible row count and mouse hit area.
    pub playlist_offset: usize,
    pub playlist_height: usize,
    pub playlist_area: Rect,
    /// 扫描进度 / scan progress
    pub discovery: (u64, u64),
    pub scan_done: bool,
    /// Debug 弹窗开关与数据 / debug popup toggle and data
    pub show_debug: bool,
    pub debug: DebugInfo,
    //AI=== 命令模式：输入缓冲、错误提示与帮助弹窗 / command mode: input buffer, error notice and help popup
    /// `:` 命令输入缓冲；None 表示未处于命令模式 / `:` command input buffer; None when command mode is off
    pub command: Option<String>,
    /// 上一条命令的错误提示，显示在底栏 / last command error, shown in the footer
    pub command_error: Option<String>,
    pub show_help: bool,
    //AI_END===
    //AI=== 左右方向键：长按期间只更新预览，松开后提交 / left/right: holds only move the preview until release
    pub seek_gesture: Option<SeekGesture>,
    //AI_END===
    //AI=== 底栏可点击区域 / clickable footer hitboxes
    pub footer_area: Rect,
    pub footer_actions: Vec<(u16, u16, FooterAction)>,
    //AI_END===
}

//AI=== 底栏按钮动作 / footer button actions
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FooterAction {
    Quit,
    Next,
    SeekBack,
    SeekForward,
    Toggle,
    ScrollUp,
    ScrollDown,
    VolumeDown,
    VolumeUp,
    Debug,
    Command,
}
//AI_END===

//AI=== seek 手势：方向、预览目标与最近一次按键时间 / seek gesture: direction, preview target and last key time
pub struct SeekGesture {
    /// 预览目标位置 / preview target
    pub target: Duration,
    /// 是否已有尚未提交的重复调整 / whether repeats are pending commit
    pub pending: bool,
    /// 最近一次方向键事件时间 / last arrow event
    pub last: Instant,
    /// -1 后退，1 前进 / -1 back, 1 forward
    pub direction: i8,
    /// 手势起始时间与起点，长按预览按持有时间推进 / hold preview advances by held time from this origin
    pub started: Instant,
    pub start: Duration,
}
//AI_END===

impl App {
    pub fn new() -> Self {
        Self {
            logs: VecDeque::new(),
            should_quit: false,
            title: String::new(),
            artist: String::new(),
            album: String::new(),
            audio_info: String::new(),
            path: String::new(),
            pos: Duration::ZERO,
            duration: Duration::ZERO,
            volume: 1.0,
            mode: String::new(),
            playing: false,
            poisoned: false,
            playlist: Vec::new(),
            current: None,
            playlist_offset: 0,
            playlist_height: 0,
            playlist_area: Rect::ZERO,
            discovery: (0, 0),
            scan_done: false,
            show_debug: false,
            debug: DebugInfo::default(),
            //AI=== 命令模式初始为空 / command mode starts empty
            command: None,
            command_error: None,
            show_help: false,
            //AI_END===
            //AI=== seek 手势初始为空 / the seek gesture starts idle
            seek_gesture: None,
            //AI_END===
            //AI=== 底栏按钮初始未布局 / footer buttons start unlaid-out
            footer_area: Rect::ZERO,
            footer_actions: Vec::new(),
            //AI_END===
        }
    }

    /// 用运行快照更新播放字段 / update playback fields from a runtime snapshot
    pub(super) fn apply(&mut self, snapshot: Snapshot) {
        let previous = self.current;
        let Snapshot {
            track,
            pos,
            volume,
            playing,
            poisoned,
            mode,
            index,
            debug,
        } = snapshot;

        self.pos = pos;
        self.volume = volume;
        self.playing = playing;
        self.poisoned = poisoned;
        self.mode = mode_label(mode).to_string();
        self.current = index;
        self.debug = debug;

        // 换曲时视口跟随当前曲目；用户手动滚动后只有换曲才会拉回
        // On a track change the viewport follows the current track; manual scrolling is only pulled back on change.
        if index != previous {
            //AI=== 换曲作废跨曲目的 seek 手势 / a track change invalidates the seek gesture
            self.seek_gesture = None;
            //AI_END===
            self.reveal_current();
        }

        if let Some(track) = track {
            self.title = track.title().to_string();
            self.artist = track.artist().to_string();
            self.album = track.album().unwrap_or("未知专辑").to_string();
            self.audio_info = if track.audio_info().is_empty() {
                "—".to_string()
            } else {
                track.audio_info().to_string()
            };
            self.path = track.path().display().to_string();
            self.duration = *track.duration();
        } else {
            self.title.clear();
            self.artist.clear();
            self.album.clear();
            self.audio_info.clear();
            self.path.clear();
            self.duration = Duration::ZERO;
        }
    }

    /// 这是 Update 逻辑的一部分：接收新日志并存入缓冲区 / receive a log line into the bounded buffer
    pub fn push_log(&mut self, line: LogLine) {
        // 保持缓冲区大小，例如最多保留 1000 行 / cap the buffer at 1000 lines
        if self.logs.len() >= 1000 {
            self.logs.pop_front();
        }
        self.logs.push_back(line);
    }

    //AI=== 播放列表滚动：键盘与滚轮共用同一套偏移夹取 / playlist scrolling shared by keys and the wheel
    /// 按行滚动播放列表 / scroll the playlist by lines
    pub fn scroll_playlist(&mut self, delta: isize) {
        let max = self.playlist_max_offset().cast_signed();
        self.playlist_offset = (self.playlist_offset.cast_signed() + delta)
            .clamp(0, max)
            .cast_unsigned();
    }

    /// 按页滚动播放列表 / scroll the playlist by pages
    pub fn scroll_playlist_page(&mut self, pages: isize) {
        let step = self.playlist_height.max(1).cast_signed();
        self.scroll_playlist(pages * step);
    }

    /// 滚动到指定偏移，越界夹到有效范围 / scroll to an offset, clamped to the valid range
    pub fn scroll_playlist_to(&mut self, offset: usize) {
        self.playlist_offset = offset.min(self.playlist_max_offset());
    }

    /// 有效偏移上限：列表长度减可见行数 / highest valid offset: length minus visible rows
    pub fn playlist_max_offset(&self) -> usize {
        self.playlist.len().saturating_sub(self.playlist_height)
    }

    /// 把当前曲目拉进视口 / bring the current track into the viewport
    fn reveal_current(&mut self) {
        let Some(index) = self.current else {
            return;
        };
        let height = self.playlist_height.max(1);
        if index < self.playlist_offset {
            self.playlist_offset = index;
        } else if index >= self.playlist_offset + height {
            self.playlist_offset = index + 1 - height;
        }
        self.scroll_playlist_to(self.playlist_offset);
    }

    /// 鼠标位置对应的播放列表索引；边框与视口外返回 None
    /// Playlist index under a mouse position; None outside the list or on the border.
    pub fn playlist_index_at(&self, column: u16, row: u16) -> Option<usize> {
        let area = self.playlist_area;
        if !area.contains(Position::new(column, row)) {
            return None;
        }
        let inner_row = row.checked_sub(area.y.saturating_add(1))?;
        if inner_row >= area.height.saturating_sub(2) {
            return None;
        }
        let index = self.playlist_offset + usize::from(inner_row);
        (index < self.playlist.len()).then_some(index)
    }
    //AI_END===

    //AI=== 进度显示值：长按预览优先 / displayed position: the hold preview wins
    pub fn display_pos(&self) -> Duration {
        self.seek_gesture
            .as_ref()
            .map_or(self.pos, |gesture| gesture.target)
    }
    //AI_END===

    //AI=== 底栏按钮命中：返回鼠标 x 命中的动作 / footer hit-test: the action under a mouse column
    pub fn footer_action_at(&self, column: u16) -> Option<FooterAction> {
        self.footer_actions
            .iter()
            .find(|(start, end, _)| (*start..*end).contains(&column))
            .map(|(_, _, action)| *action)
    }
    //AI_END===

    /// 静态预览数据（仅测试）/ static preview data, tests only
    #[cfg(test)]
    pub fn preview() -> Self {
        Self {
            playlist: vec![
                "示例曲目 01 — 示例艺术家".to_string(),
                "示例曲目 02 — 示例艺术家".to_string(),
                "示例曲目 03 — 示例艺术家".to_string(),
                "示例曲目 04 — 示例艺术家".to_string(),
                "示例曲目 05 — 示例艺术家".to_string(),
                "示例曲目 06（较长占位标题，用于测试截断）— 示例艺术家".to_string(),
                "示例曲目 07 — 示例艺术家".to_string(),
                "示例曲目 08 — 示例艺术家".to_string(),
            ],
            title: "示例曲目标题".to_string(),
            artist: "示例艺术家".to_string(),
            album: "示例专辑".to_string(),
            audio_info: "FLAC 44.1 kHz / 2ch".to_string(),
            path: "D:\\Music\\示例艺术家\\示例专辑\\01 - 示例曲目标题.flac".to_string(),
            pos: Duration::from_secs(83),
            duration: Duration::from_secs(296),
            volume: 0.75,
            mode: "顺序循环".to_string(),
            playing: true,
            current: Some(0),
            discovery: (128, 12),
            scan_done: true,
            ..Self::new()
        }
    }
}

fn mode_label(mode: LoopMode) -> &'static str {
    match mode {
        LoopMode::SequentialLoop => "顺序循环",
        LoopMode::Sequential => "顺序播放",
        LoopMode::RepeatOne => "单曲循环",
        LoopMode::Shuffle => "随机播放",
        LoopMode::PeriodicShuffle => "周期随机",
    }
}
//AI_END===
