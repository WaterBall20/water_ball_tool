//! wbfp TUI 状态 / wbfp TUI state
//!
//! `App` 按职责分为 `Ui`、`Nav`、`Jobs` 三组状态；后台任务只通过
//! [`JobEvent`] 修改任务状态。
//!
//! `App` groups its state into `Ui`, `Nav` and `Jobs`; background jobs only
//! modify job state through [`JobEvent`].

//AI=== TUI 状态、目录导航与事件归并 / TUI state, navigation and event merging
use super::browse::{self, Entry, PackInfo};
use crate::command::music_play::tui::channel_writer::{LogLevel, LogLine};
use crate::command::tui_input::{MouseState, list_index_at};
use crate::command::wbfp::common;
use crate::command::wbfp::events::{JobEvent, JobSummary, RowStatus};
use crate::command::wbfp::ops::JobSpec;
use crate::command::wbfp::{
    WaterBallFilePackCommand, WaterBallFilePackCommands, WaterBallFilePackCommandsBrowse,
};
use ratatui::layout::Rect;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, error, warn};
use water_ball_tool_lib::wb_files_pack::manager_sync::ManagerSync;

/// TUI 当前模式 / current TUI mode.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Mode {
    /// 无子命令的文件管理器 / no-subcommand file manager
    Manager,
    Browse,
    Pack,
    Unpack,
    Verify,
}

impl Mode {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Manager => "管理 / manager",
            Self::Browse => "浏览 / browse",
            Self::Pack => "打包 / pack",
            Self::Unpack => "解包 / unpack",
            Self::Verify => "校验 / verify",
        }
    }
}

/// 文件管理器焦点 / file-manager focus.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum Focus {
    #[default]
    Local,
    Pack,
}

/// 任务卡片状态 / job-card state.
#[derive(Debug, Default)]
pub(crate) struct JobState {
    pub label: String,
    pub output: String,
    pub done: u64,
    pub total: u64,
    pub files_done: u64,
    pub files_total: u64,
    pub failed: u64,
    pub warnings: u64,
    pub finished: bool,
    pub error: Option<String>,
    pub summary: Option<JobSummary>,
}

impl JobState {
    fn new(label: &str, output: String) -> Self {
        Self {
            label: label.to_string(),
            output,
            ..Self::default()
        }
    }

    pub(crate) fn ratio(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            (self.done as f64 / self.total as f64).clamp(0.0, 1.0)
        }
    }
}

/// 目录下已见状态的计数，用于父目录聚合 / counts of known descendant states for parent aggregation.
#[derive(Default)]
struct DirAgg {
    pending: u64,
    running: u64,
    done: u64,
    warning: u64,
    failed: u64,
}

impl DirAgg {
    fn add(&mut self, status: RowStatus) {
        match status {
            RowStatus::Pending => self.pending = self.pending.saturating_add(1),
            RowStatus::Running => self.running = self.running.saturating_add(1),
            RowStatus::Done => self.done = self.done.saturating_add(1),
            RowStatus::Warning => self.warning = self.warning.saturating_add(1),
            RowStatus::Failed => self.failed = self.failed.saturating_add(1),
        }
    }

    fn remove(&mut self, status: RowStatus) {
        match status {
            RowStatus::Pending => self.pending = self.pending.saturating_sub(1),
            RowStatus::Running => self.running = self.running.saturating_sub(1),
            RowStatus::Done => self.done = self.done.saturating_sub(1),
            RowStatus::Warning => self.warning = self.warning.saturating_sub(1),
            RowStatus::Failed => self.failed = self.failed.saturating_sub(1),
        }
    }

    fn status(&self) -> Option<RowStatus> {
        if self.failed > 0 {
            Some(RowStatus::Failed)
        } else if self.warning > 0 {
            Some(RowStatus::Warning)
        } else if self.running > 0 || self.pending > 0 {
            // 有未完成子项时父目录显示进行中 / a parent with unfinished children reads as running
            Some(RowStatus::Running)
        } else if self.done > 0 {
            Some(RowStatus::Done)
        } else {
            None
        }
    }
}

/// 任务列表中的一行（按文件绑定进度）/ one task-list row bound to a file's progress.
#[derive(Clone, Debug)]
pub(crate) struct TaskItem {
    pub path: PathBuf,
    pub name: String,
    pub done: u64,
    pub total: u64,
    pub status: RowStatus,
}

impl TaskItem {
    pub(crate) fn ratio(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            (self.done as f64 / self.total as f64).clamp(0.0, 1.0)
        }
    }
}

const TASK_ACTIVE_CAP: usize = 64;
const TASK_DONE_CAP: usize = 100;

/// 弹层窗口 / modal window.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum Window {
    #[default]
    None,
    Help,
    Debug,
}

/// 浏览与导航状态 / browsing and navigation state.
pub(crate) struct Nav {
    pub cwd: PathBuf,
    pub local_root: Option<PathBuf>,
    pub entries: Vec<Entry>,
    pub selected: usize,
    pub offset: usize,
    pub height: usize,
    pub show_preview: bool,
    pub list_area: Rect,
    pub mouse: MouseState,
}

impl Default for Nav {
    fn default() -> Self {
        Self {
            cwd: PathBuf::new(),
            local_root: None,
            entries: Vec::new(),
            selected: 0,
            offset: 0,
            height: 1,
            show_preview: true,
            list_area: Rect::ZERO,
            mouse: MouseState::default(),
        }
    }
}

/// 任务卡片与逐行状态 / job card and per-row state.
#[derive(Default)]
pub(crate) struct Jobs {
    pub state: Option<JobState>,
    /// 当前操作的文件（最新在最前）/ currently operating files (newest first)
    pub active: VecDeque<TaskItem>,
    /// 已完成文件（最新在最前，放在活动任务下方）/ finished files (newest first, below active)
    pub done: VecDeque<TaskItem>,
    pub running: bool,
    pub spec: Option<JobSpec>,
    pub statuses: HashMap<PathBuf, RowStatus>,
    /// 目录祖先的状态计数；只在事件到达时增量更新 / incremental descendant counts for ancestor directories
    dir_agg: HashMap<PathBuf, DirAgg>,
    /// 已发现的目录路径 / discovered directory paths
    dir_paths: HashSet<PathBuf>,
}

/// 界面壳状态 / UI shell state.
pub(crate) struct Ui {
    pub mode: Mode,
    pub title: String,
    pub pack_info: Option<PackInfo>,
    pub window: Window,
    pub logs: VecDeque<LogLine>,
    pub should_quit: bool,
    pub quit_after_job: bool,
    pub error: Option<String>,
}

/// TUI 全部可变状态 / all mutable TUI state.
pub(crate) struct App {
    pub ui: Ui,
    /// 主面板：子命令模式下的唯一面板；Manager 模式下是包视图。
    /// Primary pane: the only pane in subcommand modes; the pack pane in manager mode.
    pub nav: Nav,
    /// Manager 模式下的本地文件系统面板 / local-filesystem pane in manager mode.
    pub local_nav: Nav,
    pub focus: Focus,
    pub jobs: Jobs,
    pub pack: Option<ManagerSync>,
    /// 当前打开包的规范化路径与文件头版本（用于左侧包标记）。
    /// Canonical path and header version of the open pack (for the left-pane marker).
    pub pack_path: Option<PathBuf>,
    pub pack_format_version: Option<u16>,
}

impl App {
    fn new(mode: Mode) -> Self {
        Self {
            ui: Ui {
                mode,
                title: String::new(),
                pack_info: None,
                window: Window::None,
                logs: VecDeque::new(),
                should_quit: false,
                quit_after_job: false,
                error: None,
            },
            nav: Nav::default(),
            local_nav: Nav::default(),
            focus: Focus::Local,
            jobs: Jobs::default(),
            pack: None,
            pack_path: None,
            pack_format_version: None,
        }
    }

    /// 根据命令行参数构造 TUI；操作型命令会同时准备后台任务。
    ///
    /// Build the TUI from CLI arguments; operation commands also prepare their job.
    pub(crate) fn from_args(
        args: WaterBallFilePackCommand,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        // 无子命令：文件管理器；包路径可选，无包时只显示文件系统与预览。
        // No subcommand: file manager; the pack path is optional, and without a pack only the local pane and preview are shown.
        let Some(commands) = args.commands else {
            let mut app = Self::new(Mode::Manager);
            app.ui.title = "manager".to_string();
            app.local_nav.cwd = std::env::current_dir()?;
            app.local_nav.show_preview = app.nav.show_preview;
            app.load_local_dir().map_err(std::io::Error::other)?;
            if let Some(pack_path) = args.pack_path {
                app.open_pack(Path::new(&pack_path))
                    .map_err(std::io::Error::other)?;
            } else {
                app.focus = Focus::Local;
            }
            return Ok(app);
        };
        match commands {
            WaterBallFilePackCommands::Browse(WaterBallFilePackCommandsBrowse { pack_path }) => {
                let pack = ManagerSync::open(&pack_path)
                    .map_err(|e| format!("打开包文件错误 / failed to open pack file: {e}"))?;
                let info = browse::pack_info(&pack).map_err(std::io::Error::other)?;
                let mut app = Self::new(Mode::Browse);
                app.ui.title = pack_path;
                app.ui.pack_info = Some(info);
                app.pack = Some(pack);
                app.load_current_dir().map_err(std::io::Error::other)?;
                Ok(app)
            }
            WaterBallFilePackCommands::Pack(p) => {
                let in_path = PathBuf::from(&p.in_path);
                if !in_path.exists() {
                    return Err(format!(
                        "输入路径不存在 / input path not found: {}",
                        in_path.display()
                    )
                    .into());
                }
                let pack_path = common::pack_output_path(&in_path, p.out_pack_path.as_deref())?;
                let mut app = Self::new(Mode::Pack);
                app.ui.title = pack_path.display().to_string();
                app.nav.local_root = Some(in_path.clone());
                app.nav.cwd = in_path.clone();
                app.jobs.state = Some(JobState::new(
                    Mode::Pack.label(),
                    pack_path.display().to_string(),
                ));
                app.jobs.running = true;
                app.jobs.spec = Some(JobSpec::Pack {
                    in_path: in_path.clone(),
                    pack_path,
                    separate_manifest: !p.no_separation,
                    write_optimization: !p.no_write_optimization,
                    thread_count: common::get_thread_count(p.thread_count),
                });
                app.load_current_dir().map_err(std::io::Error::other)?;
                Ok(app)
            }
            WaterBallFilePackCommands::Unpack(u) => {
                let out_dir = common::unpack_out_dir(&u.pack_path, u.out_dir.as_deref())?;
                let pack = ManagerSync::open(&u.pack_path)
                    .map_err(|e| format!("打开包文件错误 / failed to open pack file: {e}"))?;
                let info = browse::pack_info(&pack).map_err(std::io::Error::other)?;
                let mut app = Self::new(Mode::Unpack);
                app.ui.title = u.pack_path.clone();
                app.ui.pack_info = Some(info);
                app.pack = Some(pack.clone());
                app.jobs.state = Some(JobState::new(
                    Mode::Unpack.label(),
                    out_dir.display().to_string(),
                ));
                app.jobs.running = true;
                app.jobs.spec = Some(JobSpec::Unpack {
                    pack,
                    out_dir,
                    hash_verify: u.hash_verify,
                    thread_count: common::get_thread_count(u.thread_count),
                });
                app.load_current_dir().map_err(std::io::Error::other)?;
                Ok(app)
            }
            WaterBallFilePackCommands::HashVerify(h) => {
                let pack = ManagerSync::open(&h.pack_path)
                    .map_err(|e| format!("打开包文件错误 / failed to open pack file: {e}"))?;
                let info = browse::pack_info(&pack).map_err(std::io::Error::other)?;
                let mut app = Self::new(Mode::Verify);
                app.ui.title = h.pack_path.clone();
                app.ui.pack_info = Some(info);
                app.pack = Some(pack.clone());
                // 校验没有输出目录，任务卡片不显示输出字段 / verify writes nothing, so the task card has no output field
                app.jobs.state = Some(JobState::new(Mode::Verify.label(), String::new()));
                app.jobs.running = true;
                app.jobs.spec = Some(JobSpec::Verify {
                    pack,
                    thread_count: common::get_thread_count(h.thread_count),
                });
                app.load_current_dir().map_err(std::io::Error::other)?;
                Ok(app)
            }
        }
    }

    /// Manager 模式下的活动面板 / active pane in manager mode.
    pub(crate) fn active_nav(&self) -> &Nav {
        if self.ui.mode == Mode::Manager && self.focus == Focus::Local {
            &self.local_nav
        } else {
            &self.nav
        }
    }

    pub(crate) fn active_nav_mut(&mut self) -> &mut Nav {
        if self.ui.mode == Mode::Manager && self.focus == Focus::Local {
            &mut self.local_nav
        } else {
            &mut self.nav
        }
    }

    pub(crate) fn is_manager(&self) -> bool {
        self.ui.mode == Mode::Manager
    }

    /// 只读探测并打开包，切换到包视图 / probe and open a pack, then focus the pack pane.
    pub(crate) fn open_pack(&mut self, path: &Path) -> Result<(), String> {
        if self.jobs.running {
            return Err(
                "任务运行中，不能切换包 / a job is running; cannot switch packs".to_string(),
            );
        }
        let probe = water_ball_tool_lib::wb_files_pack::probe_pack(path)
            .map_err(|e| format!("探测包失败 / failed to probe pack: {e}"))?;
        let Some(probe) = probe else {
            return Err(format!(
                "不是有效的 WBFilesPack 文件 / not a WBFilesPack file: {}",
                path.display()
            ));
        };
        // 先关闭旧包，避免同一进程的 .lock 冲突 / drop the old pack first to avoid a same-process .lock conflict
        self.close_pack();
        let pack = ManagerSync::options()
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|e| format!("打开包文件错误 / failed to open pack file: {e}"))?;
        let info = browse::pack_info(&pack)?;
        self.pack = Some(pack);
        self.ui.pack_info = Some(info);
        self.ui.title = path.display().to_string();
        self.pack_path = Some(fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()));
        self.pack_format_version = Some(probe.format_version);
        self.ui.error = None;
        self.nav.cwd = PathBuf::new();
        self.nav.entries.clear();
        self.nav.selected = 0;
        self.nav.offset = 0;
        self.focus = Focus::Pack;
        debug!(
            "open_pack: {} format_version={}",
            path.display(),
            probe.format_version
        );
        self.load_pack_dir()?;
        // 重新加载本地面板以标记正在打开的包 / reload local pane to mark the open pack
        if let Err(err) = self.load_local_dir() {
            warn!("reload local pane after opening pack failed: {err}");
            self.ui.error = Some(err);
        }
        Ok(())
    }

    /// 关闭当前包并回到本地文件系统 / close the current pack and return to the local pane.
    pub(crate) fn close_pack(&mut self) {
        if self.jobs.running {
            self.ui.error = Some(
                "任务运行中，不能关闭包 / a job is running; cannot close the pack".to_string(),
            );
            return;
        }
        self.pack = None;
        self.pack_path = None;
        self.pack_format_version = None;
        self.ui.pack_info = None;
        self.ui.title = "manager".to_string();
        self.nav.cwd = PathBuf::new();
        self.nav.entries.clear();
        self.nav.selected = 0;
        self.nav.offset = 0;
        if self.is_manager() {
            self.focus = Focus::Local;
            self.ui.error = None;
        }
    }

    fn load_local_dir(&mut self) -> Result<(), String> {
        let cwd = self.local_nav.cwd.clone();
        debug!("load_local_dir: cwd={:?}", cwd);
        let mut entries = browse::local_dir(&cwd)?;
        //AI=== 已打开包即使被自身 .lock 占用也显示标记 / mark the open pack even when its own .lock blocks a probe
        if let Some(pack_path) = self.pack_path.as_ref() {
            let version = self.pack_format_version.unwrap_or(0);
            for entry in &mut entries {
                if entry.is_dir {
                    continue;
                }
                let same = entry.path == *pack_path
                    || fs::canonicalize(&entry.path)
                        .ok()
                        .as_ref()
                        .is_some_and(|path| path == pack_path);
                if same {
                    entry.pack_version = Some(version);
                }
            }
        }
        //AI_END===
        let at_root = cwd.parent().is_none();
        self.local_nav.entries = if at_root {
            entries
        } else {
            browse::with_parent(entries, cwd.parent().unwrap_or(&cwd).to_path_buf())
        };
        self.local_nav.selected = 0;
        self.local_nav.offset = 0;
        Self::clamp_nav_offset(&mut self.local_nav);
        self.ui.error = None;
        Ok(())
    }

    fn load_pack_dir(&mut self) -> Result<(), String> {
        let Some(pack) = self.pack.as_mut() else {
            self.nav.entries.clear();
            return Ok(());
        };
        let cwd = self.nav.cwd.clone();
        debug!("load_pack_dir: cwd={:?}", cwd);
        let entries = browse::pack_dir(pack, &cwd)?;
        let at_root = cwd.as_os_str().is_empty();
        self.nav.entries = if at_root {
            entries
        } else {
            browse::with_parent(entries, cwd.parent().unwrap_or(Path::new("")).to_path_buf())
        };
        self.nav.selected = 0;
        self.nav.offset = 0;
        Self::clamp_nav_offset(&mut self.nav);
        self.ui.pack_info = browse::pack_info(pack).ok();
        self.ui.error = None;
        Ok(())
    }

    /// 在文件系统面板和包面板之间切换焦点 / switch focus between the local and pack panes.
    pub(crate) fn switch_focus(&mut self) {
        if !self.is_manager() {
            return;
        }
        self.focus = match self.focus {
            Focus::Local if self.pack.is_some() => Focus::Pack,
            Focus::Pack => Focus::Local,
            Focus::Local => Focus::Local,
        };
    }

    /// 重新读取活动面板的当前目录并回到首行 / reload the active pane's current directory and reset to the first row.
    pub(crate) fn load_current_dir(&mut self) -> Result<(), String> {
        if self.is_manager() {
            return match self.focus {
                Focus::Local => self.load_local_dir(),
                Focus::Pack => self.load_pack_dir(),
            };
        }

        debug!(
            "load_current_dir: mode={:?}, cwd={:?}, pack_open={}",
            self.ui.mode,
            self.nav.cwd,
            self.pack.is_some()
        );
        let entries = if self.ui.mode == Mode::Pack {
            if self.nav.cwd.is_file() {
                browse::local_file(&self.nav.cwd).map(|entry| vec![entry])
            } else {
                browse::local_dir(&self.nav.cwd)
            }
        } else if let Some(pack) = self.pack.as_mut() {
            browse::pack_dir(pack, &self.nav.cwd)
        } else {
            Err("包未打开 / pack is not open".to_string())
        };

        let entries = entries?;
        debug!(
            "load_current_dir: {:?} -> {} entries",
            self.nav.cwd,
            entries.len()
        );
        let at_root = self.is_at_root();
        self.nav.entries = if at_root {
            entries
        } else {
            browse::with_parent(entries, self.parent_path())
        };
        self.nav.selected = 0;
        self.nav.offset = 0;
        self.clamp_offset();
        self.ui.error = None;
        Ok(())
    }

    fn reload(&mut self) {
        if let Err(e) = self.load_current_dir() {
            warn!("load directory failed: {e}");
            self.active_nav_mut().entries.clear();
            self.ui.error = Some(e);
        }
    }

    pub(crate) fn is_at_root(&self) -> bool {
        match self.ui.mode {
            Mode::Manager if self.focus == Focus::Local => self.local_nav.cwd.parent().is_none(),
            Mode::Pack => self
                .nav
                .local_root
                .as_ref()
                .is_none_or(|root| self.nav.cwd == *root),
            _ => self.nav.cwd.as_os_str().is_empty(),
        }
    }

    fn parent_path(&self) -> PathBuf {
        match self.ui.mode {
            Mode::Manager if self.focus == Focus::Local => self
                .local_nav
                .cwd
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| self.local_nav.cwd.clone()),
            Mode::Pack => {
                let root = self.nav.local_root.as_deref().unwrap_or(Path::new(""));
                let parent = self.nav.cwd.parent().unwrap_or(root);
                if parent.starts_with(root) {
                    parent.to_path_buf()
                } else {
                    root.to_path_buf()
                }
            }
            _ => self
                .nav
                .cwd
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default(),
        }
    }

    /// 当前目录的显示文本 / display text for the current directory.
    pub(crate) fn display_cwd(&self) -> String {
        //AI=== Manager 左侧显示真实路径 / manager's left pane shows the real path
        if self.ui.mode == Mode::Manager && self.focus == Focus::Local {
            return self.local_nav.cwd.display().to_string();
        }
        //AI_END===
        let relative = if self.ui.mode == Mode::Pack {
            self.nav
                .local_root
                .as_deref()
                .and_then(|root| self.nav.cwd.strip_prefix(root).ok())
                .unwrap_or(&self.nav.cwd)
        } else {
            &self.nav.cwd
        };
        if relative.as_os_str().is_empty() {
            "/".to_string()
        } else {
            let mut out = String::from("/");
            let mut first = true;
            for component in relative.components() {
                if !first {
                    out.push('/');
                }
                out.push_str(&component.as_os_str().to_string_lossy());
                first = false;
            }
            out
        }
    }

    pub(crate) fn enter(&mut self) {
        let Some(entry) = self
            .active_nav()
            .entries
            .get(self.active_nav().selected)
            .cloned()
        else {
            debug!("enter ignored: no selected entry");
            return;
        };
        debug!(
            "enter: selected={}, name={}, path={:?}, is_dir={}, synthetic={}, pack={:?}",
            self.active_nav().selected,
            entry.name,
            entry.path,
            entry.is_dir,
            entry.synthetic,
            entry.pack_version
        );
        if entry.synthetic {
            self.back();
            return;
        }
        if !entry.is_dir {
            //AI=== Manager：本地文件按包格式探测结果决定是否打开 / manager: open local files only when probed as a pack
            if self.is_manager()
                && self.focus == Focus::Local
                && entry.pack_version.is_some()
                && let Err(err) = self.open_pack(&entry.path)
            {
                warn!("open pack failed: {err}");
                self.ui.error = Some(err);
            }
            //AI_END===
            return;
        }
        self.active_nav_mut().cwd = entry.path;
        self.reload();
    }

    pub(crate) fn back(&mut self) {
        if self.is_at_root() {
            debug!("back ignored: at root");
            return;
        }
        let parent = self.parent_path();
        debug!("back: -> {:?}", parent);
        self.active_nav_mut().cwd = parent;
        self.reload();
    }

    pub(crate) fn scroll(&mut self, delta: isize) {
        let max = self.max_offset() as isize;
        let nav = self.active_nav_mut();
        nav.offset = (nav.offset as isize + delta).clamp(0, max) as usize;
    }

    pub(crate) fn move_selection(&mut self, delta: isize) {
        let nav = self.active_nav_mut();
        if nav.entries.is_empty() {
            return;
        }
        let max = nav.entries.len().saturating_sub(1) as isize;
        nav.selected = (nav.selected as isize + delta).clamp(0, max) as usize;
        Self::reveal_nav_selected(nav);
    }

    pub(crate) fn page_selection(&mut self, pages: isize) {
        let step = self.active_nav().height.max(1) as isize;
        self.move_selection(pages * step);
    }

    pub(crate) fn select_first(&mut self) {
        let nav = self.active_nav_mut();
        nav.selected = 0;
        nav.offset = 0;
    }

    pub(crate) fn select_last(&mut self) {
        let nav = self.active_nav_mut();
        nav.selected = nav.entries.len().saturating_sub(1);
        nav.offset = Self::nav_max_offset(nav);
    }

    pub(crate) fn select_index(&mut self, index: usize) {
        let nav = self.active_nav_mut();
        if index < nav.entries.len() {
            nav.selected = index;
            Self::reveal_nav_selected(nav);
        }
    }

    fn reveal_nav_selected(nav: &mut Nav) {
        let height = nav.height.max(1);
        if nav.selected < nav.offset {
            nav.offset = nav.selected;
        } else if nav.selected >= nav.offset.saturating_add(height) {
            nav.offset = nav.selected + 1 - height;
        }
    }

    pub(crate) fn max_offset(&self) -> usize {
        Self::nav_max_offset(self.active_nav())
    }

    fn nav_max_offset(nav: &Nav) -> usize {
        nav.entries.len().saturating_sub(nav.height.max(1))
    }

    fn clamp_offset(&mut self) {
        Self::clamp_nav_offset(&mut self.nav);
    }

    fn clamp_nav_offset(nav: &mut Nav) {
        let max = Self::nav_max_offset(nav);
        if nav.offset > max {
            nav.offset = max;
        }
    }

    pub(crate) fn list_index_at(&self, column: u16, row: u16) -> Option<usize> {
        let nav = self.active_nav();
        list_index_at(nav.list_area, nav.offset, nav.entries.len(), column, row)
    }

    pub(crate) fn selected_entry(&self) -> Option<&Entry> {
        let nav = self.active_nav();
        nav.entries.get(nav.selected)
    }

    pub(crate) fn selected_size(&self) -> u64 {
        self.selected_entry().map(|e| e.size).unwrap_or(0)
    }

    /// 把当前焦点条目复制到另一面板 / copy the focused entry to the other pane.
    pub(crate) fn copy_to_other_pane(&mut self) {
        self.transfer_to_other_pane(false);
    }

    /// 把当前焦点条目移动到另一面板 / move the focused entry to the other pane.
    pub(crate) fn move_to_other_pane(&mut self) {
        self.transfer_to_other_pane(true);
    }

    fn transfer_to_other_pane(&mut self, move_entry: bool) {
        // shortcut: 单文件/目录同步执行；超大目录会阻塞 UI，后续改为后台 JobSpec::Transfer。
        // shortcut: a single file/directory is transferred synchronously; very large directories
        // block the UI. Move this to a background JobSpec::Transfer later.
        if !self.is_manager() || self.jobs.running {
            return;
        }
        let Some(entry) = self.selected_entry().cloned() else {
            return;
        };
        if entry.synthetic {
            return;
        }
        let source_local = self.focus == Focus::Local;
        let Some(mut pack) = self.pack.clone() else {
            self.ui.error = Some("没有打开包，无法跨面板复制/移动 / no pack is open".to_string());
            return;
        };
        let dest_root = if source_local {
            self.nav.cwd.clone()
        } else {
            self.local_nav.cwd.clone()
        };
        let dest = dest_root.join(&entry.name);
        let dest_exists = if source_local {
            pack.path_exists(&dest).unwrap_or(false)
        } else {
            fs::symlink_metadata(&dest).is_ok()
        };
        if dest_exists {
            self.ui.error = Some(format!(
                "目标已存在 / target already exists: {}",
                dest.display()
            ));
            return;
        }

        let result = if source_local {
            copy_local_to_pack(&mut pack, &entry.path, &dest)
        } else {
            copy_pack_to_local(&mut pack, &entry.path, &dest)
        };
        if let Err(err) = result {
            self.ui.error = Some(err);
            return;
        }

        if move_entry {
            let delete_result = if source_local {
                if entry.is_dir {
                    fs::remove_dir_all(&entry.path).map_err(|e| e.to_string())
                } else {
                    fs::remove_file(&entry.path).map_err(|e| e.to_string())
                }
            } else if entry.is_dir {
                pack.delete_dir_all(&entry.path).map_err(|e| e.to_string())
            } else {
                pack.delete_file(&entry.path).map_err(|e| e.to_string())
            };
            if let Err(err) = delete_result {
                self.ui.error = Some(format!(
                    "已复制但删除源失败 / copied but failed to delete source: {err}"
                ));
            }
        }

        // 两侧都刷新，避免旧列表继续显示已移动的条目 / reload both panes after a transfer
        if let Err(err) = self.load_local_dir() {
            self.ui.error = Some(err);
        }
        if self.pack.is_some()
            && let Err(err) = self.load_pack_dir()
        {
            self.ui.error = Some(err);
        }
    }

    pub(crate) fn status_of(&self, entry: &Entry) -> Option<RowStatus> {
        if entry.synthetic {
            return None;
        }
        let own = self.jobs.statuses.get(&entry.path).copied();
        if !entry.is_dir {
            return own;
        }
        let aggregate = self.jobs.dir_agg.get(&entry.path).and_then(DirAgg::status);
        match (own, aggregate) {
            (Some(RowStatus::Failed), _) | (_, Some(RowStatus::Failed)) => Some(RowStatus::Failed),
            (Some(RowStatus::Warning), _) | (_, Some(RowStatus::Warning)) => {
                Some(RowStatus::Warning)
            }
            (_, Some(status)) => Some(status),
            _ => own,
        }
    }

    /// 子项全部完成后，把父目录从 `Pending` 推进到 `Done`（逐级向上）。
    ///
    /// After all children finish, advance parent directories from `Pending` to `Done` (cascading upward).
    fn complete_ancestors(&mut self, path: &Path) {
        for ancestor in path.ancestors().skip(1) {
            if ancestor.as_os_str().is_empty() {
                break;
            }
            let Some(status) = self.jobs.dir_agg.get(ancestor).and_then(DirAgg::status) else {
                break;
            };
            if status != RowStatus::Done {
                break;
            }
            let known = self.jobs.dir_paths.contains(ancestor);
            let own = self.jobs.statuses.get(ancestor).copied();
            if known && matches!(own, None | Some(RowStatus::Pending | RowStatus::Running)) {
                self.set_status(ancestor.to_path_buf(), RowStatus::Done);
            } else if matches!(own, Some(RowStatus::Failed | RowStatus::Warning)) {
                break;
            }
        }
    }

    /// 任务成功结束后，收尾没有子项事件的目录（例如打包时的空目录）。
    ///
    /// After a successful job, finish directories without child events (for example empty pack source dirs).
    fn finish_pending_dirs(&mut self) {
        let pending: Vec<PathBuf> = self
            .jobs
            .dir_paths
            .iter()
            .filter(|path| matches!(self.jobs.statuses.get(*path), Some(RowStatus::Pending)))
            .filter(|path| {
                self.jobs
                    .dir_agg
                    .get(*path)
                    .is_none_or(|aggregate| aggregate.pending == 0 && aggregate.running == 0)
            })
            .cloned()
            .collect();
        for path in pending {
            if matches!(self.jobs.statuses.get(&path), Some(RowStatus::Pending)) {
                self.set_status(path.clone(), RowStatus::Done);
                self.complete_ancestors(&path);
            }
        }
    }

    /// 更新一个路径的状态，并把它同步到所有祖先目录的计数。
    ///
    /// Update one path's state and push the transition into every ancestor directory's counters.
    fn set_status(&mut self, path: PathBuf, status: RowStatus) {
        let old = self.jobs.statuses.insert(path.clone(), status);
        if old == Some(status) {
            return;
        }
        for ancestor in path.ancestors().skip(1) {
            if ancestor.as_os_str().is_empty() {
                break;
            }
            let aggregate = self.jobs.dir_agg.entry(ancestor.to_path_buf()).or_default();
            if let Some(old) = old {
                aggregate.remove(old);
            }
            aggregate.add(status);
        }
    }

    pub(crate) fn push_log(&mut self, line: LogLine) {
        if self.ui.logs.len() >= 1000 {
            self.ui.logs.pop_front();
        }
        self.ui.logs.push_back(line);
    }

    fn task_item(&self, path: &Path, total: u64) -> TaskItem {
        TaskItem {
            name: path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string()),
            path: path.to_path_buf(),
            done: 0,
            total,
            status: RowStatus::Running,
        }
    }

    fn task_start(&mut self, path: PathBuf, total: u64) {
        if self.jobs.dir_paths.contains(&path)
            || matches!(
                self.jobs.statuses.get(&path),
                Some(RowStatus::Done | RowStatus::Failed)
            )
        {
            return;
        }
        if let Some(task) = self.jobs.active.iter_mut().find(|task| task.path == path) {
            task.total = total;
            task.status = RowStatus::Running;
            return;
        }
        self.jobs.done.retain(|task| task.path != path);
        self.jobs.active.push_front(self.task_item(&path, total));
        self.jobs.active.truncate(TASK_ACTIVE_CAP);
    }

    fn task_progress(&mut self, path: PathBuf, done: u64, total: u64) {
        if self.jobs.dir_paths.contains(&path)
            || matches!(
                self.jobs.statuses.get(&path),
                Some(RowStatus::Done | RowStatus::Failed)
            )
        {
            return;
        }
        if let Some(task) = self.jobs.active.iter_mut().find(|task| task.path == path) {
            task.done = done;
            task.total = total;
            task.status = RowStatus::Running;
            return;
        }
        self.task_start(path.clone(), total);
        if let Some(task) = self.jobs.active.iter_mut().find(|task| task.path == path) {
            task.done = done;
            task.total = total;
        }
    }

    fn task_done(&mut self, path: PathBuf, status: RowStatus) {
        if self.jobs.dir_paths.contains(&path) {
            return;
        }
        let mut task = self
            .jobs
            .active
            .iter()
            .position(|task| task.path == path)
            .and_then(|index| self.jobs.active.remove(index))
            .unwrap_or_else(|| self.task_item(&path, 0));
        task.status = status;
        if status == RowStatus::Done && task.total > 0 {
            task.done = task.total;
        }
        self.jobs.done.retain(|task| task.path != path);
        self.jobs.done.push_front(task);
        self.jobs.done.truncate(TASK_DONE_CAP);
    }

    fn finish_tasks(&mut self, success: bool) {
        let status = if success {
            RowStatus::Done
        } else {
            RowStatus::Failed
        };
        while let Some(mut task) = self.jobs.active.pop_front() {
            task.status = status;
            if success && task.total > 0 {
                task.done = task.total;
            }
            self.jobs.done.push_front(task);
        }
        self.jobs.done.truncate(TASK_DONE_CAP);
    }

    /// 合并后台任务事件 / merge a background job event.
    pub(crate) fn apply_event(&mut self, event: JobEvent) {
        match event {
            JobEvent::Dir { path } => {
                self.jobs.dir_paths.insert(path.clone());
                if !matches!(
                    self.jobs.statuses.get(&path),
                    Some(RowStatus::Done | RowStatus::Warning | RowStatus::Failed)
                ) {
                    self.set_status(path, RowStatus::Pending);
                }
            }
            JobEvent::File { path, .. } => {
                if !self.jobs.statuses.contains_key(&path) {
                    self.set_status(path, RowStatus::Pending);
                }
            }
            JobEvent::Start { path, total } => {
                if !matches!(
                    self.jobs.statuses.get(&path),
                    Some(RowStatus::Done | RowStatus::Failed)
                ) {
                    self.set_status(path.clone(), RowStatus::Running);
                }
                self.task_start(path, total);
            }
            JobEvent::Progress { path, done, total } => {
                if !matches!(
                    self.jobs.statuses.get(&path),
                    Some(RowStatus::Done | RowStatus::Failed)
                ) {
                    self.set_status(path.clone(), RowStatus::Running);
                }
                self.task_progress(path, done, total);
            }
            JobEvent::Done { path, status, .. } => {
                let previous = self.jobs.statuses.get(&path).copied();
                self.set_status(path.clone(), status);
                if previous != Some(status)
                    && let Some(job) = self.jobs.state.as_mut()
                {
                    match status {
                        RowStatus::Failed => job.failed = job.failed.saturating_add(1),
                        RowStatus::Warning => job.warnings = job.warnings.saturating_add(1),
                        _ => {}
                    }
                }
                if status == RowStatus::Done {
                    self.complete_ancestors(&path);
                }
                self.task_done(path, status);
            }
            JobEvent::Total {
                done,
                total,
                files_done,
                files_total,
            } => {
                if let Some(job) = self.jobs.state.as_mut() {
                    job.done = done;
                    job.total = total;
                    job.files_done = files_done;
                    job.files_total = files_total;
                }
            }
            JobEvent::Finished { result } => {
                self.jobs.running = false;
                let success = result.is_ok();
                if let Some(job) = self.jobs.state.as_mut() {
                    job.finished = true;
                    match result {
                        Ok(summary) => {
                            job.done = job.done.max(summary.bytes);
                            job.files_done = job.files_done.max(summary.files_done);
                            // 事件里的逐行状态可能比汇总更完整（例如写入优化跳过）/ row events may know more than the summary
                            job.failed = job.failed.max(summary.failed);
                            job.warnings = job.warnings.max(summary.warnings);
                            job.summary = Some(summary);
                        }
                        Err(e) => {
                            error!("wbfp job failed: {e}");
                            job.error = Some(e);
                        }
                    }
                }
                if success {
                    self.finish_pending_dirs();
                }
                self.finish_tasks(success);
                if self.ui.quit_after_job {
                    self.ui.should_quit = true;
                }
            }
        }
    }

    pub(crate) fn last_logs(&self, count: usize) -> Vec<(Option<LogLevel>, &str)> {
        let skip = self.ui.logs.len().saturating_sub(count);
        self.ui
            .logs
            .iter()
            .skip(skip)
            .map(|line| (line.level, line.text.as_str()))
            .collect()
    }
}

//AI=== 本地文件系统与包之间的复制 / copy between the local filesystem and a pack
fn copy_local_to_pack(pack: &mut ManagerSync, src: &Path, dst: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(src)
        .map_err(|e| format!("读取源文件失败 / failed to stat source: {e}"))?;
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "跳过符号链接 / symlink is not supported: {}",
            src.display()
        ));
    }
    if metadata.is_dir() {
        pack.create_dir_all(&dst)
            .map_err(|e| format!("创建包内目录失败 / failed to create pack directory: {e}"))?;
        for entry in fs::read_dir(src).map_err(|e| format!("读取源目录失败: {e}"))? {
            let entry = entry.map_err(|e| format!("读取源目录项失败: {e}"))?;
            let name = entry.file_name();
            copy_local_to_pack(pack, &entry.path(), &dst.join(name))?;
        }
        Ok(())
    } else if metadata.is_file() {
        let mut input = fs::File::open(src)
            .map_err(|e| format!("打开源文件失败 / failed to open source: {e}"))?;
        let mut output = pack
            .virtual_file_options()
            .write(true)
            .create_new(true)
            .alloc_size(Some(metadata.len()))
            .open(dst)
            .map_err(|e| format!("创建包内文件失败 / failed to create pack file: {e}"))?;
        output
            .set_len(metadata.len())
            .map_err(|e| format!("设置包内文件长度失败: {e}"))?;
        let modified = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |duration| duration.as_millis());
        output
            .set_modified(modified)
            .map_err(|e| format!("设置包内文件修改时间失败: {e}"))?;
        std::io::copy(&mut input, &mut output)
            .map_err(|e| format!("复制到包失败 / failed to copy into pack: {e}"))?;
        Ok(())
    } else {
        Err(format!(
            "不支持的源文件类型 / unsupported source type: {}",
            src.display()
        ))
    }
}

fn copy_pack_to_local(pack: &mut ManagerSync, src: &Path, dst: &Path) -> Result<(), String> {
    let item = pack
        .get_pack_struct_item(&src)
        .map_err(|e| format!("读取包内条目失败 / failed to read pack entry: {e}"))?;
    match item.item_type() {
        water_ball_tool_lib::wb_files_pack::PackStructItemType::Dir { .. } => {
            fs::create_dir_all(dst)
                .map_err(|e| format!("创建本地目录失败 / failed to create local directory: {e}"))?;
            let names = pack
                .get_struct_item_name_list(&src)
                .map_err(|e| format!("读取包内目录失败 / failed to read pack directory: {e}"))?;
            for name in names {
                copy_pack_to_local(pack, &src.join(&name), &dst.join(&name))?;
            }
            Ok(())
        }
        water_ball_tool_lib::wb_files_pack::PackStructItemType::File { .. } => {
            let mut buf = vec![0u8; 1024 * 1024];
            common::extract_pack_file_to_disk(pack, src, dst, &mut buf, None)
                .map_err(|e| format!("解包到本地失败 / failed to extract: {e}"))?;
            Ok(())
        }
    }
}
//AI_END===

//AI=== 测试拆分到独立文件 / tests split into separate files
#[cfg(test)]
mod browse_test;
#[cfg(test)]
mod test;
#[cfg(test)]
mod unpack_job_test;
//AI_END===
