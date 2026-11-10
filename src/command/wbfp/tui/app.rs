//! wbfp TUI 状态 / wbfp TUI state
//!
//! `App` 以 `Ui`、`Nav`、`Jobs` 为核心，另有目标面板、待执行传输、重命名与删除确认等临时状态；
//! 后台任务只通过 [`JobEvent`] 修改任务状态。
//!
//! `App` is centered on `Ui`, `Nav` and `Jobs`, with transient state for the
//! destination picker, pending transfer, inline rename and delete confirmation;
//! background jobs only modify job state through [`JobEvent`].

//AI=== TUI 状态、目录导航与事件归并 / TUI state, navigation and event merging
const MANAGER_TWO_PANE_MIN_WIDTH: u16 = 80;

use super::browse::{self, Entry, PackInfo};
use crate::command::music_play::tui::channel_writer::{LogLevel, LogLine};
use crate::command::tui_input::{MouseState, list_index_at};
use crate::command::wbfp::common;
use crate::command::wbfp::events::{JobEvent, JobSummary, RowStatus};
use crate::command::wbfp::ops::{self, JobSpec, TransferEndpoint};
use crate::command::wbfp::{
    WaterBallFilePackCommand, WaterBallFilePackCommands, WaterBallFilePackCommandsBrowse,
};
use ratatui::crossterm::event::{KeyCode, KeyModifiers};
use ratatui::layout::Rect;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::thread::JoinHandle;
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
    /// 删除确认 / delete confirmation
    Confirm,
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

/// 跨系统待执行传输 / pending cross-namespace transfer.
pub(crate) struct PendingTransfer {
    pub source_local: bool,
    pub source: Entry,
    pub move_entry: bool,
}

/// 同系统目标选择器 / same-namespace destination picker.
pub(crate) struct Picker {
    pub move_entry: bool,
    pub source_local: bool,
    pub source: Entry,
    pub nav: Nav,
}

/// 原地重命名编辑 / inline rename editor.
pub(crate) struct RenameEdit {
    pub source_local: bool,
    pub path: PathBuf,
    pub original: String,
    pub name: String,
    /// 光标（按字符计）/ cursor measured in characters
    pub cursor: usize,
    pub selected_all: bool,
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

impl Jobs {
    /// 清空上一次任务，准备启动新任务 / clear the previous job before starting a new one.
    fn reset(&mut self) {
        self.state = None;
        self.active.clear();
        self.done.clear();
        self.running = false;
        self.spec = None;
        self.statuses.clear();
        self.dir_agg.clear();
        self.dir_paths.clear();
    }
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
    /// 跨系统待执行传输（窄屏切换对方列表后按 `v`）/ pending cross-pane transfer.
    pub pending: Option<PendingTransfer>,
    /// 同系统目标选择器 / same-namespace destination picker.
    pub picker: Option<Picker>,
    /// 原地重命名编辑 / inline rename editor.
    pub rename: Option<RenameEdit>,
    /// 等待确认删除的条目（命名空间，条目）/ entry awaiting delete confirmation (namespace, entry).
    pub delete: Option<(bool, Entry)>,
    /// 最近一帧的终端宽度 / terminal width of the latest frame.
    pub frame_width: u16,
    /// 后台任务事件发送端（由 TUI 主循环注入）/ job event sender injected by the TUI loop.
    pub job_tx: Option<Sender<JobEvent>>,
    /// 已启动任务线程 / spawned job threads.
    pub job_handles: Vec<JoinHandle<()>>,
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
            pending: None,
            picker: None,
            rename: None,
            delete: None,
            frame_width: 120,
            job_tx: None,
            job_handles: Vec::new(),
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
            app.load_local_dir().map_err(io::Error::other)?;
            if let Some(pack_path) = args.pack_path {
                app.open_pack(Path::new(&pack_path))
                    .map_err(io::Error::other)?;
            } else {
                app.focus = Focus::Local;
            }
            return Ok(app);
        };
        match commands {
            WaterBallFilePackCommands::Browse(WaterBallFilePackCommandsBrowse { pack_path }) => {
                let pack = ManagerSync::open(&pack_path)
                    .map_err(|e| format!("打开包文件错误 / failed to open pack file: {e}"))?;
                let info = browse::pack_info(&pack).map_err(io::Error::other)?;
                let mut app = Self::new(Mode::Browse);
                app.ui.title = pack_path;
                app.ui.pack_info = Some(info);
                app.pack = Some(pack);
                app.load_current_dir().map_err(io::Error::other)?;
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
                app.nav.cwd.clone_from(&in_path);
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
                app.load_current_dir().map_err(io::Error::other)?;
                Ok(app)
            }
            WaterBallFilePackCommands::Unpack(u) => {
                let out_dir = common::unpack_out_dir(&u.pack_path, u.out_dir.as_deref())?;
                let pack = ManagerSync::open(&u.pack_path)
                    .map_err(|e| format!("打开包文件错误 / failed to open pack file: {e}"))?;
                let info = browse::pack_info(&pack).map_err(io::Error::other)?;
                let mut app = Self::new(Mode::Unpack);
                app.ui.title.clone_from(&u.pack_path);
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
                app.load_current_dir().map_err(io::Error::other)?;
                Ok(app)
            }
            WaterBallFilePackCommands::HashVerify(h) => {
                let pack = ManagerSync::open(&h.pack_path)
                    .map_err(|e| format!("打开包文件错误 / failed to open pack file: {e}"))?;
                let info = browse::pack_info(&pack).map_err(io::Error::other)?;
                let mut app = Self::new(Mode::Verify);
                app.ui.title.clone_from(&h.pack_path);
                app.ui.pack_info = Some(info);
                app.pack = Some(pack.clone());
                // 校验没有输出目录，任务卡片不显示输出字段 / verify writes nothing, so the task card has no output field
                app.jobs.state = Some(JobState::new(Mode::Verify.label(), String::new()));
                app.jobs.running = true;
                app.jobs.spec = Some(JobSpec::Verify {
                    pack,
                    thread_count: common::get_thread_count(h.thread_count),
                });
                app.load_current_dir().map_err(io::Error::other)?;
                Ok(app)
            }
        }
    }

    /// 注入后台任务事件发送端 / inject the job event sender.
    pub(crate) fn set_job_sender(&mut self, tx: Sender<JobEvent>) {
        self.job_tx = Some(tx);
    }

    /// 启动 `from_args` 准备的任务 / start the job prepared by `from_args`.
    pub(crate) fn start_pending_job(&mut self) -> io::Result<()> {
        let Some(spec) = self.jobs.spec.take() else {
            return Ok(());
        };
        self.spawn(spec)
    }

    /// 取出任务线程以便退出时 join / take job threads so the caller can join them.
    pub(crate) fn take_job_handles(&mut self) -> Vec<JoinHandle<()>> {
        std::mem::take(&mut self.job_handles)
    }

    fn spawn(&mut self, spec: JobSpec) -> io::Result<()> {
        let tx = self.job_tx.clone().ok_or_else(|| {
            io::Error::other("后台任务通道未初始化 / job channel is not initialized")
        })?;
        match ops::spawn(spec, tx) {
            Ok(handle) => {
                self.job_handles.push(handle);
                Ok(())
            }
            Err(e) => {
                self.jobs.running = false;
                let message = format!("启动后台任务失败 / failed to start job: {e}");
                error!("{message}");
                if let Some(job) = self.jobs.state.as_mut() {
                    job.finished = true;
                    job.error = Some(message.clone());
                }
                self.ui.error = Some(message);
                Err(e)
            }
        }
    }

    fn both_panes_visible(&self) -> bool {
        self.pack.is_some() && self.frame_width >= MANAGER_TWO_PANE_MIN_WIDTH
    }

    fn transfer_spec(
        &self,
        source_local: bool,
        dest_local: bool,
        source: &Entry,
        dest: PathBuf,
        move_entry: bool,
    ) -> Option<JobSpec> {
        let pack = self.pack.clone();
        let source_endpoint = if source_local {
            TransferEndpoint::Local(source.path.clone())
        } else {
            TransferEndpoint::Pack {
                pack: pack.clone()?,
                path: source.path.clone(),
            }
        };
        let dest_endpoint = if dest_local {
            TransferEndpoint::Local(dest)
        } else {
            TransferEndpoint::Pack {
                pack: pack?,
                path: dest,
            }
        };
        Some(JobSpec::Transfer {
            source: source_endpoint,
            dest: dest_endpoint,
            move_entry,
            thread_count: common::get_thread_count(None),
        })
    }

    fn transfer_output(&self, dest_local: bool, dest: &Path) -> String {
        if dest_local {
            dest.display().to_string()
        } else {
            format!("{}:{}", self.ui.title, dest.display())
        }
    }

    /// 启动后台复制/移动任务；目标已存在或参数无效时返回 `false`。
    /// Start a background copy/move job; returns `false` when the target exists or input is invalid.
    fn start_transfer_job(
        &mut self,
        source_local: bool,
        dest_local: bool,
        source: &Entry,
        dest: PathBuf,
        move_entry: bool,
    ) -> bool {
        if source_local == dest_local && dest == source.path {
            self.ui.error = Some("目标与源相同 / target is the same as the source".to_string());
            return false;
        }
        let exists = if dest_local {
            fs::symlink_metadata(&dest).is_ok()
        } else {
            self.pack
                .as_mut()
                .is_some_and(|pack| pack.path_exists(&dest).unwrap_or(false))
        };
        if exists {
            self.ui.error = Some(format!(
                "目标已存在 / target already exists: {}",
                dest.display()
            ));
            return false;
        }
        let Some(spec) =
            self.transfer_spec(source_local, dest_local, source, dest.clone(), move_entry)
        else {
            self.ui.error = Some("没有打开包 / no pack is open".to_string());
            return false;
        };
        let label = if move_entry {
            "移动 / Move"
        } else {
            "复制 / Copy"
        };
        let output = self.transfer_output(dest_local, &dest);
        self.jobs.reset();
        self.jobs.state = Some(JobState::new(label, output));
        self.jobs.running = true;
        if let Err(e) = self.spawn(spec) {
            debug!("spawn transfer job failed: {e}");
        }
        true
    }

    /// Manager 模式下的活动面板 / active pane in manager mode.
    pub(crate) fn active_nav(&self) -> &Nav {
        if let Some(picker) = &self.picker {
            return &picker.nav;
        }
        if self.ui.mode == Mode::Manager && self.focus == Focus::Local {
            &self.local_nav
        } else {
            &self.nav
        }
    }

    pub(crate) fn active_nav_mut(&mut self) -> &mut Nav {
        if let Some(picker) = self.picker.as_mut() {
            return &mut picker.nav;
        }
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
        self.pending = None;
        self.picker = None;
        self.rename = None;
        self.delete = None;
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

    fn load_picker_dir(&mut self) -> Result<(), String> {
        let (source_local, cwd) = match self.picker.as_ref() {
            Some(picker) => (picker.source_local, picker.nav.cwd.clone()),
            None => return Ok(()),
        };
        debug!("load_picker_dir: local={source_local}, cwd={cwd:?}");
        let entries = if source_local {
            browse::local_dir(&cwd)?
        } else {
            let Some(pack) = self.pack.as_mut() else {
                return Err("包未打开 / pack is not open".to_string());
            };
            browse::pack_dir(pack, &cwd)?
        };
        let at_root = if source_local {
            cwd.parent().is_none()
        } else {
            cwd.as_os_str().is_empty()
        };
        let entries = if at_root {
            entries
        } else if source_local {
            browse::with_parent(
                entries,
                cwd.parent().map_or_else(|| cwd.clone(), Path::to_path_buf),
            )
        } else {
            browse::with_parent(entries, cwd.parent().unwrap_or(Path::new("")).to_path_buf())
        };
        if let Some(picker) = self.picker.as_mut() {
            picker.nav.entries = entries;
            picker.nav.selected = 0;
            picker.nav.offset = 0;
            Self::clamp_nav_offset(&mut picker.nav);
        }
        self.ui.error = None;
        Ok(())
    }

    /// 在文件系统面板和包面板之间切换焦点 / switch focus between the local and pack panes.
    pub(crate) fn switch_focus(&mut self) {
        if !self.is_manager() || self.picker.is_some() || self.pending.is_some() {
            return;
        }
        self.focus = match self.focus {
            Focus::Local if self.pack.is_some() => Focus::Pack,
            Focus::Pack | Focus::Local => Focus::Local,
        };
    }

    /// 重新读取活动面板的当前目录并回到首行 / reload the active pane's current directory and reset to the first row.
    pub(crate) fn load_current_dir(&mut self) -> Result<(), String> {
        if self.picker.is_some() {
            return self.load_picker_dir();
        }
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
        if let Some(picker) = &self.picker {
            return if picker.source_local {
                picker.nav.cwd.parent().is_none()
            } else {
                picker.nav.cwd.as_os_str().is_empty()
            };
        }
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
        if let Some(picker) = &self.picker {
            return if picker.source_local {
                picker
                    .nav
                    .cwd
                    .parent()
                    .map_or_else(|| picker.nav.cwd.clone(), Path::to_path_buf)
            } else {
                picker
                    .nav
                    .cwd
                    .parent()
                    .unwrap_or(Path::new(""))
                    .to_path_buf()
            };
        }
        match self.ui.mode {
            Mode::Manager if self.focus == Focus::Local => self
                .local_nav
                .cwd
                .parent()
                .map_or_else(|| self.local_nav.cwd.clone(), Path::to_path_buf),
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
        if let Some(picker) = &self.picker {
            return if picker.source_local {
                picker.nav.cwd.display().to_string()
            } else {
                Self::pack_display(&picker.nav.cwd)
            };
        }
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
        Self::pack_display(relative)
    }

    fn pack_display(relative: &Path) -> String {
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

    /// 顶部操作横幅：当前操作与源路径 / top banner: current action and source path.
    pub(crate) fn operation_banner(&self) -> Option<String> {
        if let Some(picker) = &self.picker {
            return Some(format!(
                "{}  源 / src: {} {}",
                if picker.move_entry {
                    "[移动 / MOVE]"
                } else {
                    "[复制 / COPY]"
                },
                if picker.source_local {
                    "[文件系统 / FS]"
                } else {
                    "[包 / PACK]"
                },
                picker.source.path.display()
            ));
        }
        if let Some(pending) = &self.pending {
            return Some(format!(
                "{}  源 / src: {} {}",
                if pending.move_entry {
                    "[移动 / MOVE]"
                } else {
                    "[复制 / COPY]"
                },
                if pending.source_local {
                    "[文件系统 / FS]"
                } else {
                    "[包 / PACK]"
                },
                pending.source.path.display()
            ));
        }
        if let Some(rename) = &self.rename {
            return Some(format!("[重命名 / RENAME]  {}", rename.path.display()));
        }
        None
    }

    pub(crate) fn enter(&mut self) {
        if self.picker.is_some() {
            self.enter_picker();
            return;
        }
        // 待执行传输只允许在目标列表内导航，不打开文件 / a pending transfer only navigates the target list
        if self.pending.is_some() {
            let Some(entry) = self.selected_entry().cloned() else {
                return;
            };
            if entry.synthetic {
                self.back();
            } else if entry.is_dir {
                self.active_nav_mut().cwd = entry.path;
                self.reload();
            }
            return;
        }
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
        self.selected_entry().map_or(0, |e| e.size)
    }

    /// 同系统复制：打开目标选择器 / same-namespace copy: open the destination picker.
    pub(crate) fn copy_same_namespace(&mut self) {
        self.start_picker(false);
    }

    /// 同系统移动：打开目标选择器 / same-namespace move: open the destination picker.
    pub(crate) fn move_same_namespace(&mut self) {
        self.start_picker(true);
    }

    /// 跨系统复制 / cross-namespace copy.
    pub(crate) fn copy_cross_namespace(&mut self) {
        self.cross_transfer(false);
    }

    /// 跨系统移动 / cross-namespace move.
    pub(crate) fn move_cross_namespace(&mut self) {
        self.cross_transfer(true);
    }

    fn start_picker(&mut self, move_entry: bool) {
        if !self.is_manager()
            || self.jobs.running
            || self.picker.is_some()
            || self.pending.is_some()
        {
            return;
        }
        let Some(entry) = self.selected_entry().cloned() else {
            return;
        };
        if entry.synthetic {
            return;
        }
        let source_local = self.focus == Focus::Local;
        if !source_local && self.pack.is_none() {
            return;
        }
        let start = entry
            .path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        let mut nav = Nav::default();
        nav.cwd = start;
        nav.show_preview = self.nav.show_preview;
        self.picker = Some(Picker {
            move_entry,
            source_local,
            source: entry.clone(),
            nav,
        });
        if let Err(err) = self.load_picker_dir() {
            self.ui.error = Some(err);
            self.picker = None;
            return;
        }
        if let Some(picker) = self.picker.as_mut()
            && let Some(index) = picker
                .nav
                .entries
                .iter()
                .position(|item| item.path == entry.path)
        {
            picker.nav.selected = index;
            Self::reveal_nav_selected(&mut picker.nav);
        }
    }

    fn enter_picker(&mut self) {
        let Some(entry) = self.selected_entry().cloned() else {
            return;
        };
        if entry.synthetic {
            self.back();
            return;
        }
        if !entry.is_dir {
            return;
        }
        if self.picker_forbidden(&entry.path) {
            self.ui.error = Some(
                "不能进入源目录的子树 / cannot enter a directory inside the source".to_string(),
            );
            return;
        }
        self.active_nav_mut().cwd = entry.path;
        self.reload();
    }

    /// 目标是源目录本身或其后代（仅目录源）/ target is the source directory or one of its descendants.
    fn picker_forbidden(&self, candidate: &Path) -> bool {
        let Some(picker) = &self.picker else {
            return false;
        };
        if !picker.source.is_dir {
            return false;
        }
        candidate == picker.source.path || candidate.starts_with(&picker.source.path)
    }

    pub(crate) fn confirm_picker(&mut self) {
        if self.jobs.running {
            return;
        }
        let Some((source_local, move_entry, source, dest_dir)) =
            self.picker.as_ref().map(|picker| {
                (
                    picker.source_local,
                    picker.move_entry,
                    picker.source.clone(),
                    picker.nav.cwd.clone(),
                )
            })
        else {
            return;
        };
        if source.is_dir && (dest_dir == source.path || dest_dir.starts_with(&source.path)) {
            self.ui.error =
                Some("不能进入源目录的子树 / cannot use a directory inside the source".to_string());
            return;
        }
        let dest = dest_dir.join(&source.name);
        if !self.start_transfer_job(source_local, source_local, &source, dest, move_entry) {
            return;
        }
        self.picker = None;
        self.focus = if source_local {
            Focus::Local
        } else {
            Focus::Pack
        };
    }

    pub(crate) fn cancel_picker(&mut self) {
        if let Some(picker) = self.picker.take() {
            self.focus = if picker.source_local {
                Focus::Local
            } else {
                Focus::Pack
            };
            self.ui.error = None;
        }
    }

    fn cross_transfer(&mut self, move_entry: bool) {
        if !self.is_manager()
            || self.jobs.running
            || self.picker.is_some()
            || self.pending.is_some()
        {
            return;
        }
        let Some(entry) = self.selected_entry().cloned() else {
            return;
        };
        if entry.synthetic {
            return;
        }
        if self.pack.is_none() {
            self.ui.error = Some("没有打开包，无法跨文件系统操作 / no pack is open".to_string());
            return;
        }
        let source_local = self.focus == Focus::Local;
        if self.both_panes_visible() {
            let dest_dir = if source_local {
                self.nav.cwd.clone()
            } else {
                self.local_nav.cwd.clone()
            };
            let dest = dest_dir.join(&entry.name);
            self.start_transfer_job(source_local, !source_local, &entry, dest, move_entry);
        } else {
            self.pending = Some(PendingTransfer {
                source_local,
                source: entry,
                move_entry,
            });
            self.focus = if source_local {
                Focus::Pack
            } else {
                Focus::Local
            };
            self.reload();
        }
    }

    pub(crate) fn confirm_pending(&mut self) {
        if self.jobs.running {
            return;
        }
        let Some(pending) = self.pending.take() else {
            return;
        };
        let dest_dir = if pending.source_local {
            self.nav.cwd.clone()
        } else {
            self.local_nav.cwd.clone()
        };
        let dest = dest_dir.join(&pending.source.name);
        if !self.start_transfer_job(
            pending.source_local,
            !pending.source_local,
            &pending.source,
            dest,
            pending.move_entry,
        ) {
            self.pending = Some(pending);
        }
    }

    pub(crate) fn cancel_pending(&mut self) {
        if let Some(pending) = self.pending.take() {
            self.focus = if pending.source_local {
                Focus::Local
            } else {
                Focus::Pack
            };
            self.ui.error = None;
        }
    }

    /// 开始原地重命名 / start inline rename of the selected entry.
    pub(crate) fn start_rename(&mut self) {
        if !self.is_manager()
            || self.jobs.running
            || self.picker.is_some()
            || self.pending.is_some()
        {
            return;
        }
        let Some(entry) = self.selected_entry().cloned() else {
            return;
        };
        if entry.synthetic {
            return;
        }
        let source_local = self.focus == Focus::Local;
        self.rename = Some(RenameEdit {
            source_local,
            path: entry.path,
            original: entry.name.clone(),
            name: entry.name,
            cursor: 0,
            selected_all: true,
        });
        if let Some(edit) = self.rename.as_mut() {
            edit.cursor = edit.name.chars().count();
        }
    }

    /// 重命名输入框的按键处理；返回 `true` 表示已消费该按键。
    /// Handle one key in the rename editor; returns `true` when the key was consumed.
    pub(crate) fn rename_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> bool {
        if self.rename.is_none() {
            return false;
        }
        match code {
            KeyCode::Enter => self.commit_rename(),
            KeyCode::Esc => self.cancel_rename(),
            _ => {
                let Some(edit) = self.rename.as_mut() else {
                    return false;
                };
                match code {
                    KeyCode::Char('a') if modifiers.contains(KeyModifiers::CONTROL) => {
                        edit.selected_all = true;
                    }
                    KeyCode::Char(c)
                        if !modifiers.contains(KeyModifiers::CONTROL)
                            && !modifiers.contains(KeyModifiers::ALT) =>
                    {
                        if edit.selected_all {
                            edit.name.clear();
                            edit.cursor = 0;
                            edit.selected_all = false;
                        }
                        let index = char_to_byte(&edit.name, edit.cursor);
                        edit.name.insert(index, c);
                        edit.cursor = edit.cursor.saturating_add(1);
                    }
                    KeyCode::Backspace => {
                        if edit.selected_all {
                            edit.name.clear();
                            edit.cursor = 0;
                            edit.selected_all = false;
                        } else if edit.cursor > 0 {
                            edit.cursor = edit.cursor.saturating_sub(1);
                            let index = char_to_byte(&edit.name, edit.cursor);
                            edit.name.remove(index);
                        }
                    }
                    KeyCode::Delete => {
                        if edit.selected_all {
                            edit.name.clear();
                            edit.cursor = 0;
                            edit.selected_all = false;
                        } else {
                            let index = char_to_byte(&edit.name, edit.cursor);
                            if index < edit.name.len() {
                                edit.name.remove(index);
                            }
                        }
                    }
                    KeyCode::Left => {
                        edit.selected_all = false;
                        edit.cursor = edit.cursor.saturating_sub(1);
                    }
                    KeyCode::Right => {
                        edit.selected_all = false;
                        let max = edit.name.chars().count();
                        edit.cursor = edit.cursor.saturating_add(1).min(max);
                    }
                    KeyCode::Home => {
                        edit.selected_all = false;
                        edit.cursor = 0;
                    }
                    KeyCode::End => {
                        edit.selected_all = false;
                        edit.cursor = edit.name.chars().count();
                    }
                    _ => {}
                }
            }
        }
        true
    }

    fn commit_rename(&mut self) {
        let Some(edit) = self.rename.take() else {
            return;
        };
        let name = edit.name.trim().to_string();
        if name == edit.original {
            return;
        }
        if let Err(message) = validate_leaf_name(&name) {
            self.ui.error = Some(message);
            self.rename = Some(edit);
            return;
        }
        let Some(parent) = edit.path.parent().map(Path::to_path_buf) else {
            self.ui.error =
                Some("无法获取父目录 / cannot resolve the parent directory".to_string());
            self.rename = Some(edit);
            return;
        };
        let dest = parent.join(&name);
        if edit.source_local {
            let dest_exists = fs::symlink_metadata(&dest).is_ok();
            // 仅大小写变化时规范路径相同，允许重命名 / a case-only rename resolves to the same file
            let same_file = dest_exists
                && fs::canonicalize(&dest)
                    .ok()
                    .zip(fs::canonicalize(&edit.path).ok())
                    .is_some_and(|(dest, source)| dest == source);
            if dest_exists && !same_file {
                self.ui.error = Some(format!(
                    "目标已存在 / target already exists: {}",
                    dest.display()
                ));
                self.rename = Some(edit);
                return;
            }
            if let Err(err) = fs::rename(&edit.path, &dest) {
                self.ui.error = Some(format!("重命名失败 / rename failed: {err}"));
                self.rename = Some(edit);
                return;
            }
        } else {
            let Some(pack) = self.pack.as_mut() else {
                self.ui.error = Some("包未打开 / pack is not open".to_string());
                self.rename = Some(edit);
                return;
            };
            if let Err(err) = pack.rename(&edit.path, &dest) {
                self.ui.error = Some(format!("重命名失败 / rename failed: {err}"));
                self.rename = Some(edit);
                return;
            }
        }
        self.rename = None;
        self.focus = if edit.source_local {
            Focus::Local
        } else {
            Focus::Pack
        };
        self.reload();
        if let Some(index) = self
            .active_nav()
            .entries
            .iter()
            .position(|entry| entry.path == dest)
        {
            self.select_index(index);
        }
    }

    pub(crate) fn cancel_rename(&mut self) {
        self.rename = None;
        self.ui.error = None;
    }

    /// 请求删除当前条目（弹窗确认）/ request deletion of the selected entry (confirmation popup).
    pub(crate) fn request_delete(&mut self) {
        if !self.is_manager()
            || self.jobs.running
            || self.picker.is_some()
            || self.pending.is_some()
        {
            return;
        }
        let Some(entry) = self.selected_entry().cloned() else {
            return;
        };
        if entry.synthetic {
            return;
        }
        let source_local = self.focus == Focus::Local;
        self.delete = Some((source_local, entry));
        self.ui.window = Window::Confirm;
    }

    pub(crate) fn confirm_delete(&mut self) {
        self.ui.window = Window::None;
        let Some((source_local, entry)) = self.delete.take() else {
            return;
        };
        let result = if source_local {
            if entry.is_dir {
                fs::remove_dir_all(&entry.path)
            } else {
                fs::remove_file(&entry.path)
            }
            .map_err(|err| err.to_string())
        } else if let Some(pack) = self.pack.as_mut() {
            if entry.is_dir {
                pack.delete_dir_all(&entry.path)
            } else {
                pack.delete_file(&entry.path)
            }
            .map_err(|err| err.to_string())
        } else {
            Err("包未打开 / pack is not open".to_string())
        };
        if let Err(err) = result {
            self.ui.error = Some(format!("删除失败 / delete failed: {err}"));
            return;
        }
        self.reload();
    }

    pub(crate) fn cancel_delete(&mut self) {
        self.delete = None;
        self.ui.window = Window::None;
    }

    /// `Esc` 优先级：弹窗 → 重命名 → 目标面板 → 待执行传输；未取消任何状态时返回 `false`。
    /// Esc precedence: popup → rename → picker → pending; returns `false` when nothing was cancelled.
    pub(crate) fn cancel_transient(&mut self) -> bool {
        if self.ui.window != Window::None {
            if self.ui.window == Window::Confirm {
                self.cancel_delete();
            } else {
                self.ui.window = Window::None;
            }
            return true;
        }
        if self.rename.is_some() {
            self.cancel_rename();
            return true;
        }
        if self.picker.is_some() {
            self.cancel_picker();
            return true;
        }
        if self.pending.is_some() {
            self.cancel_pending();
            return true;
        }
        false
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
            name: path.file_name().map_or_else(
                || path.display().to_string(),
                |name| name.to_string_lossy().into_owned(),
            ),
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
                //AI=== 管理器任务结束后刷新两侧列表 / refresh both manager panes after a job
                if self.is_manager() {
                    if let Err(e) = self.load_local_dir() {
                        self.ui.error = Some(e);
                    }
                    if self.pack.is_some()
                        && let Err(e) = self.load_pack_dir()
                    {
                        self.ui.error = Some(e);
                    }
                }
                //AI_END===
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

//AI=== 重命名辅助 / rename helpers
fn char_to_byte(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .nth(char_index)
        .map_or(text.len(), |(index, _)| index)
}

fn validate_leaf_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("名称不能为空 / name must not be empty".to_string());
    }
    if name == "." || name == ".." {
        return Err("名称不能是 . 或 .. / name must not be . or ..".to_string());
    }
    if name.contains('/') || name.contains('\\') || name.contains('\0') {
        return Err("名称不能包含路径分隔符 / name must not contain a path separator".to_string());
    }
    Ok(())
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
