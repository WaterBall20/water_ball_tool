use super::channel_writer::LogLine;
use crate::command::music_play::runtime::{DebugInfo, Snapshot};
use crate::command::tui_input::{MouseState, list_index_at};
//AI=== 播放列表滚动与鼠标命中需要 Rect/Position / playlist scrolling and mouse hit-testing need Rect/Position
use ratatui::layout::Rect;
//AI_END===
use std::collections::VecDeque;
//AI=== 左右方向键 seek 手势需要时间戳 / left/right seek gestures need timestamps
use std::time::{Duration, Instant};
//AI_END===
use water_ball_tool_lib::music_play::LoopMode;
use water_ball_tool_lib::music_play::effect::EffectSummary;

//AI=== 当前窗口：互斥显示，None 表示主界面 / active window: mutually exclusive, None is the main view
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Window {
    #[default]
    None,
    Help,
    Debug,
    Effects,
}
//AI_END===

//AI=== TUI 状态按职责分组 / TUI state grouped by responsibility
pub struct App {
    /// 日志缓冲区，限制最大行数避免内存无限增长 / bounded log buffer
    pub logs: VecDeque<LogLine>,
    /// 标记是否应该退出，由事件循环处理 / quit flag handled by the event loop
    pub should_quit: bool,
    /// 当前曲目与播放状态 / current track and playback state
    pub playback: PlaybackState,
    /// 播放列表与视口 / playlist and viewport
    pub playlist: PlaylistState,
    /// 扫描进度 / scan progress
    pub scan: ScanState,
    /// 左右方向键 seek 手势 / left/right seek gesture
    pub seek: SeekState,
    /// 底栏布局与可点击区域 / footer layout and clickable areas
    pub footer: FooterState,
    /// 鼠标双击状态 / mouse double-click state
    pub mouse: MouseState,
    /// `:` 命令输入 / `:` command input
    pub command: CommandState,
    /// 当前显示的窗口 / active window
    pub window: Window,
    /// Debug 弹窗数据 / debug popup data
    pub debug: DebugInfo,
    /// 效果器窗口列表与选择 / effects-window list and selection
    pub effects: EffectsState,
}

/// 当前曲目信息与播放字段 / current track info and playback fields
#[derive(Default)]
pub struct PlaybackState {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub audio_info: String,
    pub path: String,
    pub pos: Duration,
    pub duration: Duration,
    /// 音量（0.0–1.0）/ volume as a 0.0–1.0 fraction
    pub volume: f32,
    /// 循环模式显示名 / loop mode display name
    pub mode: String,
    pub playing: bool,
    /// 解码器毒化（显示异常暂停）/ decoder poisoned (shows the error state)
    pub poisoned: bool,
}

/// 播放列表显示行与视口 / playlist display lines and viewport
#[derive(Default)]
pub struct PlaylistState {
    pub tracks: Vec<String>,
    pub current: Option<usize>,
    pub offset: usize,
    pub height: usize,
    pub area: Rect,
}

/// 扫描进度 / scan progress
#[derive(Default)]
pub struct ScanState {
    pub discovery: (u64, u64),
    pub done: bool,
}

/// 左右方向键 seek 手势状态 / left/right seek gesture state
#[derive(Default)]
pub struct SeekState {
    pub gesture: Option<SeekGesture>,
}

/// 底栏布局与可点击按钮 / footer layout and clickable buttons
#[derive(Default)]
pub struct FooterState {
    pub area: Rect,
    pub actions: Vec<(u16, u16, FooterAction)>,
}

/// 命令输入状态 / command input state
#[derive(Default)]
pub struct CommandState {
    /// `:` 命令输入缓冲；None 表示未处于命令模式 / `:` command input buffer; None when command mode is off
    pub input: Option<String>,
    /// 上一条命令的错误提示，显示在底栏 / last command error, shown in the footer
    pub error: Option<String>,
}

/// 效果器窗口列表与左栏选择 / effects-window list and left-column selection
#[derive(Default)]
pub struct EffectsState {
    pub list: Vec<EffectSummary>,
    pub selected: usize,
}
//AI_END===

//AI=== 底栏按钮动作 / footer button actions
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FooterAction {
    Quit,
    Next,
    Toggle,
    Debug,
    Effects,
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
            playback: PlaybackState::new(),
            playlist: PlaylistState::new(),
            scan: ScanState::default(),
            seek: SeekState::default(),
            footer: FooterState::new(),
            mouse: MouseState::default(),
            command: CommandState::default(),
            window: Window::None,
            debug: DebugInfo::default(),
            effects: EffectsState::default(),
        }
    }

    /// 用运行快照更新播放字段 / update playback fields from a runtime snapshot
    pub(super) fn apply(&mut self, snapshot: Snapshot) {
        let previous = self.playlist.current;
        let Snapshot {
            track,
            pos,
            volume,
            playing,
            poisoned,
            mode,
            index,
            debug,
            effects,
        } = snapshot;

        self.playback.pos = pos;
        self.playback.volume = volume;
        self.playback.playing = playing;
        self.playback.poisoned = poisoned;
        self.playback.mode = mode_label(mode).to_string();
        self.playlist.current = index;
        self.debug = debug;
        //AI=== 只在打开效果器窗口时刷新列表 / refresh the list only while its window is open
        if let Some(effects) = effects {
            self.effects.list = effects;
            self.effects.selected = self
                .effects
                .selected
                .min(self.effects.list.len().saturating_sub(1));
        }
        //AI_END===

        // 换曲时视口跟随当前曲目；用户手动滚动后只有换曲才会拉回
        // On a track change the viewport follows the current track; manual scrolling is only pulled back on change.
        if index != previous {
            //AI=== 换曲作废跨曲目的 seek 手势 / a track change invalidates the seek gesture
            self.seek.gesture = None;
            //AI_END===
            self.reveal_current();
        }

        if let Some(track) = track {
            self.playback.title = track.title().to_string();
            self.playback.artist = track.artist().to_string();
            self.playback.album = track.album().unwrap_or("未知专辑").to_string();
            self.playback.audio_info = if track.audio_info().is_empty() {
                "—".to_string()
            } else {
                track.audio_info().to_string()
            };
            self.playback.path = track.path().display().to_string();
            self.playback.duration = *track.duration();
        } else {
            self.playback.title.clear();
            self.playback.artist.clear();
            self.playback.album.clear();
            self.playback.audio_info.clear();
            self.playback.path.clear();
            self.playback.duration = Duration::ZERO;
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
        self.playlist.offset = (self.playlist.offset.cast_signed() + delta)
            .clamp(0, max)
            .cast_unsigned();
    }

    /// 按页滚动播放列表 / scroll the playlist by pages
    pub fn scroll_playlist_page(&mut self, pages: isize) {
        let step = self.playlist.height.max(1).cast_signed();
        self.scroll_playlist(pages * step);
    }

    /// 滚动到指定偏移，越界夹到有效范围 / scroll to an offset, clamped to the valid range
    pub fn scroll_playlist_to(&mut self, offset: usize) {
        self.playlist.offset = offset.min(self.playlist_max_offset());
    }

    /// 有效偏移上限：列表长度减可见行数 / highest valid offset: length minus visible rows
    pub fn playlist_max_offset(&self) -> usize {
        self.playlist
            .tracks
            .len()
            .saturating_sub(self.playlist.height)
    }

    /// 把当前曲目拉进视口 / bring the current track into the viewport
    fn reveal_current(&mut self) {
        let Some(index) = self.playlist.current else {
            return;
        };
        let height = self.playlist.height.max(1);
        if index < self.playlist.offset {
            self.playlist.offset = index;
        } else if index >= self.playlist.offset + height {
            self.playlist.offset = index + 1 - height;
        }
        self.scroll_playlist_to(self.playlist.offset);
    }

    /// 鼠标位置对应的播放列表索引；边框与视口外返回 None
    /// Playlist index under a mouse position; None outside the list or on the border.
    pub fn playlist_index_at(&self, column: u16, row: u16) -> Option<usize> {
        list_index_at(
            self.playlist.area,
            self.playlist.offset,
            self.playlist.tracks.len(),
            column,
            row,
        )
    }

    //AI=== 窗口互斥：同一时间只显示一个 / windows are mutually exclusive: only one is visible at a time
    /// 切换 Debug 窗口 / toggle the debug window
    pub fn toggle_debug(&mut self) {
        self.window = if self.window == Window::Debug {
            Window::None
        } else {
            Window::Debug
        };
    }

    /// 切换效果器窗口 / toggle the effects window
    pub fn toggle_effects(&mut self) {
        self.window = if self.window == Window::Effects {
            Window::None
        } else {
            Window::Effects
        };
    }

    /// 打开帮助窗口 / open the help window
    pub fn open_help(&mut self) {
        self.window = Window::Help;
    }

    /// 关闭当前窗口 / close the active window
    pub fn close_window(&mut self) {
        self.window = Window::None;
    }
    //AI_END===

    //AI=== 效果器左栏选择：方向键上下移动，越界夹取 / effects selection: arrow keys move and clamp at the ends
    /// 移动效果器选择 / move the effect selection
    pub fn select_effect(&mut self, delta: isize) {
        let max = self.effects.list.len().saturating_sub(1);
        self.effects.selected = self.effects.selected.saturating_add_signed(delta).min(max);
    }
    //AI_END===

    //AI=== 进度显示值：长按预览优先 / displayed position: the hold preview wins
    pub fn display_pos(&self) -> Duration {
        self.seek
            .gesture
            .as_ref()
            .map_or(self.playback.pos, |gesture| gesture.target)
    }
    //AI_END===

    //AI=== 底栏按钮命中：返回鼠标 x 命中的动作 / footer hit-test: the action under a mouse column
    pub fn footer_action_at(&self, column: u16) -> Option<FooterAction> {
        self.footer
            .actions
            .iter()
            .find(|(start, end, _)| (*start..*end).contains(&column))
            .map(|(_, _, action)| *action)
    }
    //AI_END===

    /// 静态预览数据（仅测试）/ static preview data, tests only
    #[cfg(test)]
    pub fn preview() -> Self {
        Self {
            playlist: PlaylistState {
                tracks: vec![
                    "示例曲目 01 — 示例艺术家".to_string(),
                    "示例曲目 02 — 示例艺术家".to_string(),
                    "示例曲目 03 — 示例艺术家".to_string(),
                    "示例曲目 04 — 示例艺术家".to_string(),
                    "示例曲目 05 — 示例艺术家".to_string(),
                    "示例曲目 06（较长占位标题，用于测试截断）— 示例艺术家".to_string(),
                    "示例曲目 07 — 示例艺术家".to_string(),
                    "示例曲目 08 — 示例艺术家".to_string(),
                ],
                current: Some(0),
                ..Default::default()
            },
            playback: PlaybackState {
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
                ..Default::default()
            },
            scan: ScanState {
                discovery: (128, 12),
                done: true,
            },
            ..Self::new()
        }
    }
}

impl PlaybackState {
    fn new() -> Self {
        Self {
            volume: 1.0,
            ..Default::default()
        }
    }
}

impl PlaylistState {
    fn new() -> Self {
        Self {
            area: Rect::ZERO,
            ..Default::default()
        }
    }
}

impl FooterState {
    fn new() -> Self {
        Self {
            area: Rect::ZERO,
            actions: Vec::new(),
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
