//! wbfp TUI 状态 / wbfp TUI state
//!
//! 列表、视口、预览、任务进度与日志都集中在 `App`；后台任务只通过
//! [`JobEvent`] 修改它。
//!
//! The list, viewport, preview, job progress and logs live in `App`; background
//! jobs only modify it through [`JobEvent`].

//AI=== TUI 状态、目录导航与事件归并 / TUI state, navigation and event merging
use super::browse::{self, Entry, PackInfo};
use crate::command::music_play::tui::channel_writer::{LogLevel, LogLine};
use crate::command::wbfp::common;
use crate::command::wbfp::events::{JobEvent, JobSummary, RowStatus};
use crate::command::wbfp::ops::JobSpec;
use crate::command::wbfp::{
    WaterBallFilePackCommand, WaterBallFilePackCommands, WaterBallFilePackCommandsBrowse,
};
use ratatui::layout::{Position, Rect};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::Instant;
use tracing::{debug, error, warn};
use water_ball_tool_lib::wb_files_pack::manager_sync::ManagerSync;

/// TUI 当前模式 / current TUI mode.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Mode {
    Browse,
    Pack,
    Unpack,
    Verify,
}

impl Mode {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Browse => "浏览 / browse",
            Self::Pack => "打包 / pack",
            Self::Unpack => "解包 / unpack",
            Self::Verify => "校验 / verify",
        }
    }
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

/// TUI 全部可变状态 / all mutable TUI state.
pub(crate) struct App {
    pub mode: Mode,
    pub title: String,
    pub pack_info: Option<PackInfo>,
    pub cwd: PathBuf,
    pub local_root: Option<PathBuf>,
    pub entries: Vec<Entry>,
    pub selected: usize,
    pub offset: usize,
    pub height: usize,
    pub show_preview: bool,
    pub statuses: HashMap<PathBuf, RowStatus>,
    /// 目录祖先的状态计数；只在事件到达时增量更新 / incremental descendant counts for ancestor directories
    dir_agg: HashMap<PathBuf, DirAgg>,
    /// 已发现的目录路径 / discovered directory paths
    dir_paths: HashSet<PathBuf>,
    pub job: Option<JobState>,
    /// 当前操作的文件（最新在最前）/ currently operating files (newest first)
    pub active_tasks: VecDeque<TaskItem>,
    /// 已完成文件（最新在最前，放在活动任务下方）/ finished files (newest first, below active)
    pub done_tasks: VecDeque<TaskItem>,
    pub job_running: bool,
    pub should_quit: bool,
    pub quit_after_job: bool,
    pub show_help: bool,
    pub show_logs: bool,
    pub logs: VecDeque<LogLine>,
    pub list_area: Rect,
    /// 双击检测：上一次左键点击的时间与行号 / double-click detection: last left-click time and row
    pub last_click: Option<(Instant, usize)>,
    pub pack: Option<ManagerSync>,
    pub error: Option<String>,
    pub job_spec: Option<JobSpec>,
}

impl App {
    fn new(mode: Mode) -> Self {
        Self {
            mode,
            title: String::new(),
            pack_info: None,
            cwd: PathBuf::new(),
            local_root: None,
            entries: Vec::new(),
            selected: 0,
            offset: 0,
            height: 1,
            show_preview: true,
            statuses: HashMap::new(),
            dir_agg: HashMap::new(),
            dir_paths: HashSet::new(),
            job: None,
            active_tasks: VecDeque::new(),
            done_tasks: VecDeque::new(),
            job_running: false,
            should_quit: false,
            quit_after_job: false,
            show_help: false,
            show_logs: false,
            logs: VecDeque::new(),
            list_area: Rect::ZERO,
            last_click: None,
            pack: None,
            error: None,
            job_spec: None,
        }
    }

    /// 根据命令行参数构造 TUI；操作型命令会同时准备后台任务。
    ///
    /// Build the TUI from CLI arguments; operation commands also prepare their job.
    pub(crate) fn from_args(
        args: WaterBallFilePackCommand,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        match args.commands {
            WaterBallFilePackCommands::Browse(WaterBallFilePackCommandsBrowse { pack_path }) => {
                let pack = ManagerSync::open(&pack_path)
                    .map_err(|e| format!("打开包文件错误 / failed to open pack file: {e}"))?;
                let info = browse::pack_info(&pack).map_err(std::io::Error::other)?;
                let mut app = Self::new(Mode::Browse);
                app.title = pack_path;
                app.pack_info = Some(info);
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
                app.title = pack_path.display().to_string();
                app.local_root = Some(in_path.clone());
                app.cwd = in_path.clone();
                app.job = Some(JobState::new(
                    Mode::Pack.label(),
                    pack_path.display().to_string(),
                ));
                app.job_running = true;
                app.job_spec = Some(JobSpec::Pack {
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
                app.title = u.pack_path.clone();
                app.pack_info = Some(info);
                app.pack = Some(pack.clone());
                app.job = Some(JobState::new(
                    Mode::Unpack.label(),
                    out_dir.display().to_string(),
                ));
                app.job_running = true;
                app.job_spec = Some(JobSpec::Unpack {
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
                app.title = h.pack_path.clone();
                app.pack_info = Some(info);
                app.pack = Some(pack.clone());
                // 校验没有输出目录，任务卡片不显示输出字段 / verify writes nothing, so the task card has no output field
                app.job = Some(JobState::new(Mode::Verify.label(), String::new()));
                app.job_running = true;
                app.job_spec = Some(JobSpec::Verify {
                    pack,
                    thread_count: common::get_thread_count(h.thread_count),
                });
                app.load_current_dir().map_err(std::io::Error::other)?;
                Ok(app)
            }
        }
    }

    /// 重新读取当前目录并回到首行 / reload the current directory and reset to the first row.
    pub(crate) fn load_current_dir(&mut self) -> Result<(), String> {
        debug!(
            "load_current_dir: mode={:?}, cwd={:?}, pack_open={}",
            self.mode,
            self.cwd,
            self.pack.is_some()
        );
        let entries = if self.mode == Mode::Pack {
            if self.cwd.is_file() {
                browse::local_file(&self.cwd).map(|entry| vec![entry])
            } else {
                browse::local_dir(&self.cwd)
            }
        } else if let Some(pack) = self.pack.as_mut() {
            browse::pack_dir(pack, &self.cwd)
        } else {
            Err("包未打开 / pack is not open".to_string())
        };

        let entries = entries?;
        debug!(
            "load_current_dir: {:?} -> {} entries",
            self.cwd,
            entries.len()
        );
        let at_root = self.is_at_root();
        self.entries = if at_root {
            entries
        } else {
            browse::with_parent(entries, self.parent_path())
        };
        self.selected = 0;
        self.offset = 0;
        self.clamp_offset();
        self.error = None;
        Ok(())
    }

    fn reload(&mut self) {
        if let Err(e) = self.load_current_dir() {
            warn!("load directory failed: {e}");
            self.entries.clear();
            self.error = Some(e);
        }
    }

    pub(crate) fn is_at_root(&self) -> bool {
        match self.mode {
            Mode::Pack => self
                .local_root
                .as_ref()
                .is_none_or(|root| self.cwd == *root),
            _ => self.cwd.as_os_str().is_empty(),
        }
    }

    fn parent_path(&self) -> PathBuf {
        match self.mode {
            Mode::Pack => {
                let root = self.local_root.as_deref().unwrap_or(Path::new(""));
                let parent = self.cwd.parent().unwrap_or(root);
                if parent.starts_with(root) {
                    parent.to_path_buf()
                } else {
                    root.to_path_buf()
                }
            }
            _ => self.cwd.parent().map(Path::to_path_buf).unwrap_or_default(),
        }
    }

    /// 当前目录的显示文本 / display text for the current directory.
    pub(crate) fn display_cwd(&self) -> String {
        let relative = if self.mode == Mode::Pack {
            self.local_root
                .as_deref()
                .and_then(|root| self.cwd.strip_prefix(root).ok())
                .unwrap_or(&self.cwd)
        } else {
            &self.cwd
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
        let Some(entry) = self.entries.get(self.selected) else {
            debug!("enter ignored: no selected entry");
            return;
        };
        debug!(
            "enter: selected={}, name={}, path={:?}, is_dir={}, synthetic={}",
            self.selected, entry.name, entry.path, entry.is_dir, entry.synthetic
        );
        if entry.synthetic {
            self.back();
            return;
        }
        if !entry.is_dir {
            return;
        }
        self.cwd = entry.path.clone();
        self.reload();
    }

    pub(crate) fn back(&mut self) {
        if self.is_at_root() {
            debug!("back ignored: at root");
            return;
        }
        let parent = self.parent_path();
        debug!("back: {:?} -> {:?}", self.cwd, parent);
        self.cwd = parent;
        self.reload();
    }

    pub(crate) fn scroll(&mut self, delta: isize) {
        let max = self.max_offset() as isize;
        self.offset = (self.offset as isize + delta).clamp(0, max) as usize;
    }

    pub(crate) fn move_selection(&mut self, delta: isize) {
        if self.entries.is_empty() {
            return;
        }
        let max = self.entries.len().saturating_sub(1) as isize;
        self.selected = (self.selected as isize + delta).clamp(0, max) as usize;
        self.reveal_selected();
    }

    pub(crate) fn page_selection(&mut self, pages: isize) {
        let step = self.height.max(1) as isize;
        self.move_selection(pages * step);
    }

    pub(crate) fn select_first(&mut self) {
        self.selected = 0;
        self.offset = 0;
    }

    pub(crate) fn select_last(&mut self) {
        self.selected = self.entries.len().saturating_sub(1);
        self.offset = self.max_offset();
    }

    pub(crate) fn select_index(&mut self, index: usize) {
        if index < self.entries.len() {
            self.selected = index;
            self.reveal_selected();
        }
    }

    fn reveal_selected(&mut self) {
        let height = self.height.max(1);
        if self.selected < self.offset {
            self.offset = self.selected;
        } else if self.selected >= self.offset.saturating_add(height) {
            self.offset = self.selected + 1 - height;
        }
        self.clamp_offset();
    }

    pub(crate) fn max_offset(&self) -> usize {
        self.entries.len().saturating_sub(self.height.max(1))
    }

    fn clamp_offset(&mut self) {
        if self.offset > self.max_offset() {
            self.offset = self.max_offset();
        }
    }

    pub(crate) fn list_index_at(&self, column: u16, row: u16) -> Option<usize> {
        let area = self.list_area;
        if !area.contains(Position::new(column, row)) {
            return None;
        }
        let inner_row = row.checked_sub(area.y.saturating_add(1))?;
        if inner_row >= area.height.saturating_sub(2) {
            return None;
        }
        let index = self.offset + usize::from(inner_row);
        (index < self.entries.len()).then_some(index)
    }

    pub(crate) fn selected_entry(&self) -> Option<&Entry> {
        self.entries.get(self.selected)
    }

    pub(crate) fn selected_size(&self) -> u64 {
        self.selected_entry().map(|e| e.size).unwrap_or(0)
    }

    pub(crate) fn status_of(&self, entry: &Entry) -> Option<RowStatus> {
        if entry.synthetic {
            return None;
        }
        let own = self.statuses.get(&entry.path).copied();
        if !entry.is_dir {
            return own;
        }
        let aggregate = self.dir_agg.get(&entry.path).and_then(DirAgg::status);
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
            let Some(status) = self.dir_agg.get(ancestor).and_then(DirAgg::status) else {
                break;
            };
            if status != RowStatus::Done {
                break;
            }
            let known = self.dir_paths.contains(ancestor);
            let own = self.statuses.get(ancestor).copied();
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
            .dir_paths
            .iter()
            .filter(|path| matches!(self.statuses.get(*path), Some(RowStatus::Pending)))
            .filter(|path| {
                self.dir_agg
                    .get(*path)
                    .is_none_or(|aggregate| aggregate.pending == 0 && aggregate.running == 0)
            })
            .cloned()
            .collect();
        for path in pending {
            if matches!(self.statuses.get(&path), Some(RowStatus::Pending)) {
                self.set_status(path.clone(), RowStatus::Done);
                self.complete_ancestors(&path);
            }
        }
    }

    /// 更新一个路径的状态，并把它同步到所有祖先目录的计数。
    ///
    /// Update one path's state and push the transition into every ancestor directory's counters.
    fn set_status(&mut self, path: PathBuf, status: RowStatus) {
        let old = self.statuses.insert(path.clone(), status);
        if old == Some(status) {
            return;
        }
        for ancestor in path.ancestors().skip(1) {
            if ancestor.as_os_str().is_empty() {
                break;
            }
            let aggregate = self.dir_agg.entry(ancestor.to_path_buf()).or_default();
            if let Some(old) = old {
                aggregate.remove(old);
            }
            aggregate.add(status);
        }
    }

    pub(crate) fn push_log(&mut self, line: LogLine) {
        if self.logs.len() >= 1000 {
            self.logs.pop_front();
        }
        self.logs.push_back(line);
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
        if self.dir_paths.contains(&path)
            || matches!(
                self.statuses.get(&path),
                Some(RowStatus::Done | RowStatus::Failed)
            )
        {
            return;
        }
        if let Some(task) = self.active_tasks.iter_mut().find(|task| task.path == path) {
            task.total = total;
            task.status = RowStatus::Running;
            return;
        }
        self.done_tasks.retain(|task| task.path != path);
        self.active_tasks.push_front(self.task_item(&path, total));
        self.active_tasks.truncate(TASK_ACTIVE_CAP);
    }

    fn task_progress(&mut self, path: PathBuf, done: u64, total: u64) {
        if self.dir_paths.contains(&path)
            || matches!(
                self.statuses.get(&path),
                Some(RowStatus::Done | RowStatus::Failed)
            )
        {
            return;
        }
        if let Some(task) = self.active_tasks.iter_mut().find(|task| task.path == path) {
            task.done = done;
            task.total = total;
            task.status = RowStatus::Running;
            return;
        }
        self.task_start(path.clone(), total);
        if let Some(task) = self.active_tasks.iter_mut().find(|task| task.path == path) {
            task.done = done;
            task.total = total;
        }
    }

    fn task_done(&mut self, path: PathBuf, status: RowStatus) {
        if self.dir_paths.contains(&path) {
            return;
        }
        let mut task = self
            .active_tasks
            .iter()
            .position(|task| task.path == path)
            .and_then(|index| self.active_tasks.remove(index))
            .unwrap_or_else(|| self.task_item(&path, 0));
        task.status = status;
        if status == RowStatus::Done && task.total > 0 {
            task.done = task.total;
        }
        self.done_tasks.retain(|task| task.path != path);
        self.done_tasks.push_front(task);
        self.done_tasks.truncate(TASK_DONE_CAP);
    }

    fn finish_tasks(&mut self, success: bool) {
        let status = if success {
            RowStatus::Done
        } else {
            RowStatus::Failed
        };
        while let Some(mut task) = self.active_tasks.pop_front() {
            task.status = status;
            if success && task.total > 0 {
                task.done = task.total;
            }
            self.done_tasks.push_front(task);
        }
        self.done_tasks.truncate(TASK_DONE_CAP);
    }

    /// 合并后台任务事件 / merge a background job event.
    pub(crate) fn apply_event(&mut self, event: JobEvent) {
        match event {
            JobEvent::Dir { path } => {
                self.dir_paths.insert(path.clone());
                if !matches!(
                    self.statuses.get(&path),
                    Some(RowStatus::Done | RowStatus::Warning | RowStatus::Failed)
                ) {
                    self.set_status(path, RowStatus::Pending);
                }
            }
            JobEvent::File { path, .. } => {
                if !self.statuses.contains_key(&path) {
                    self.set_status(path, RowStatus::Pending);
                }
            }
            JobEvent::Start { path, total } => {
                if !matches!(
                    self.statuses.get(&path),
                    Some(RowStatus::Done | RowStatus::Failed)
                ) {
                    self.set_status(path.clone(), RowStatus::Running);
                }
                self.task_start(path, total);
            }
            JobEvent::Progress { path, done, total } => {
                if !matches!(
                    self.statuses.get(&path),
                    Some(RowStatus::Done | RowStatus::Failed)
                ) {
                    self.set_status(path.clone(), RowStatus::Running);
                }
                self.task_progress(path, done, total);
            }
            JobEvent::Done { path, status, .. } => {
                let previous = self.statuses.get(&path).copied();
                self.set_status(path.clone(), status);
                if previous != Some(status)
                    && let Some(job) = self.job.as_mut()
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
                if let Some(job) = self.job.as_mut() {
                    job.done = done;
                    job.total = total;
                    job.files_done = files_done;
                    job.files_total = files_total;
                }
            }
            JobEvent::Finished { result } => {
                self.job_running = false;
                let success = result.is_ok();
                if let Some(job) = self.job.as_mut() {
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
                if self.quit_after_job {
                    self.should_quit = true;
                }
            }
        }
    }

    pub(crate) fn last_logs(&self, count: usize) -> Vec<(Option<LogLevel>, &str)> {
        let skip = self.logs.len().saturating_sub(count);
        self.logs
            .iter()
            .skip(skip)
            .map(|line| (line.level, line.text.as_str()))
            .collect()
    }
}
//AI_END===

#[cfg(test)]
impl App {
    /// 测试用静态状态 / static state for tests.
    pub(super) fn preview() -> Self {
        let mut app = Self::new(Mode::Browse);
        app.title = "D:\\packs\\game.wbfp".to_string();
        app.pack_info = Some(PackInfo {
            version: 1,
            file_count: 3,
            dir_count: 1,
            data_len: 13_312,
            cow: false,
        });
        app.cwd = PathBuf::from("assets");
        app.entries = vec![
            Entry {
                name: "..".to_string(),
                path: PathBuf::new(),
                is_dir: true,
                size: 0,
                modified: 0,
                hash: None,
                synthetic: true,
            },
            Entry {
                name: "fonts".to_string(),
                path: PathBuf::from("assets/fonts"),
                is_dir: true,
                size: 0,
                modified: 0,
                hash: None,
                synthetic: false,
            },
            Entry {
                name: "hero_idle.png".to_string(),
                path: PathBuf::from("assets/hero_idle.png"),
                is_dir: false,
                size: 12_684,
                modified: 1_761_000_000_000,
                hash: Some((1, vec![0x9f, 0x2c, 0xa7, 0x1b])),
                synthetic: false,
            },
        ];
        app.statuses
            .insert(PathBuf::from("assets/hero_idle.png"), RowStatus::Done);
        app.selected = 2;
        app
    }

    /// 校验任务卡片的静态状态（无输出字段）/ static verify job card (no output field).
    pub(super) fn preview_verify() -> Self {
        let mut app = Self::preview();
        app.mode = Mode::Verify;
        app.job = Some(JobState::new(Mode::Verify.label(), String::new()));
        app.job_running = false;
        app
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn selection_moves_and_clamps() {
        let mut app = App::preview();
        app.height = 3;
        app.move_selection(1);
        assert_eq!(app.selected, 2);
        app.move_selection(10);
        assert_eq!(app.selected, 2);
        app.select_first();
        assert_eq!(app.selected, 0);
        app.select_last();
        assert_eq!(app.selected, 2);
        assert_eq!(app.offset, 0);
    }

    #[test]
    fn job_events_fill_row_status_and_counts() {
        let mut app = App::preview();
        app.job = Some(JobState::new("test", String::new()));
        let path = PathBuf::from("assets/fonts/new.ttf");
        app.apply_event(JobEvent::Start {
            path: path.clone(),
            total: 5,
        });
        assert_eq!(app.statuses.get(&path), Some(&RowStatus::Running));
        app.apply_event(JobEvent::Done {
            path: path.clone(),
            status: RowStatus::Failed,
        });
        assert_eq!(app.statuses.get(&path), Some(&RowStatus::Failed));
        assert_eq!(app.job.as_ref().map(|job| job.failed), Some(1));
    }

    #[test]
    fn directory_status_aggregates_descendants() {
        let mut app = App::preview();
        let fonts = app.entries.iter().position(|entry| entry.name == "fonts");
        let Some(fonts) = fonts else {
            panic!("preview fixture is missing the fonts directory");
        };

        let first = PathBuf::from("assets/fonts/a.ttf");
        app.apply_event(JobEvent::File {
            path: first.clone(),
        });
        assert_eq!(app.status_of(&app.entries[fonts]), Some(RowStatus::Running));
        app.apply_event(JobEvent::Done {
            path: first,
            status: RowStatus::Done,
        });
        assert_eq!(app.status_of(&app.entries[fonts]), Some(RowStatus::Done));

        let second = PathBuf::from("assets/fonts/b.ttf");
        app.apply_event(JobEvent::File {
            path: second.clone(),
        });
        assert_eq!(app.status_of(&app.entries[fonts]), Some(RowStatus::Running));
        app.apply_event(JobEvent::Done {
            path: second,
            status: RowStatus::Failed,
        });
        assert_eq!(app.status_of(&app.entries[fonts]), Some(RowStatus::Failed));
    }

    #[test]
    fn pending_child_directory_keeps_parent_running() {
        let mut app = App::preview();
        let fonts = app.entries.iter().position(|entry| entry.name == "fonts");
        let Some(fonts) = fonts else {
            panic!("preview fixture is missing the fonts directory");
        };
        let nested = PathBuf::from("assets/fonts/nested");
        app.apply_event(JobEvent::Dir {
            path: nested.clone(),
        });
        assert_eq!(app.status_of(&app.entries[fonts]), Some(RowStatus::Running));
        app.apply_event(JobEvent::Done {
            path: nested,
            status: RowStatus::Done,
        });
        assert_eq!(app.status_of(&app.entries[fonts]), Some(RowStatus::Done));
    }

    #[test]
    fn task_list_keeps_active_files_first_and_updates_by_path() {
        let mut app = App::preview();
        let first = PathBuf::from("assets/fonts/a.ttf");
        let second = PathBuf::from("assets/fonts/b.ttf");

        app.apply_event(JobEvent::Start {
            path: first.clone(),
            total: 100,
        });
        app.apply_event(JobEvent::Start {
            path: second.clone(),
            total: 200,
        });
        assert_eq!(app.active_tasks.len(), 2);
        assert_eq!(app.active_tasks[0].path, second);
        assert_eq!(app.active_tasks[1].path, first);

        app.apply_event(JobEvent::Progress {
            path: second.clone(),
            done: 50,
            total: 200,
        });
        assert_eq!(app.active_tasks[0].done, 50);
        assert_eq!(app.done_tasks.len(), 0);

        app.apply_event(JobEvent::Done {
            path: first,
            status: RowStatus::Done,
        });
        assert_eq!(app.active_tasks.len(), 1);
        assert_eq!(app.done_tasks.len(), 1);
        assert_eq!(app.done_tasks[0].status, RowStatus::Done);
    }

    #[test]
    fn task_list_skips_directories_and_finishes_active_on_job_end() {
        let mut app = App::preview();
        app.job = Some(JobState::new("test", String::new()));
        let dir = PathBuf::from("assets/fonts");
        app.apply_event(JobEvent::Dir { path: dir.clone() });
        app.apply_event(JobEvent::Start {
            path: dir.clone(),
            total: 0,
        });
        app.apply_event(JobEvent::Done {
            path: dir,
            status: RowStatus::Done,
        });
        assert!(app.active_tasks.is_empty());
        assert!(app.done_tasks.is_empty());

        let file = PathBuf::from("assets/fonts/c.ttf");
        app.apply_event(JobEvent::Start {
            path: file,
            total: 10,
        });
        app.apply_event(JobEvent::Finished {
            result: Ok(Default::default()),
        });
        assert!(app.active_tasks.is_empty());
        assert_eq!(app.done_tasks.len(), 1);
        assert_eq!(app.done_tasks[0].status, RowStatus::Done);
    }

    #[test]
    fn display_cwd_is_relative_to_local_root() {
        let mut app = App::new(Mode::Pack);
        app.local_root = Some(PathBuf::from("src"));
        app.cwd = PathBuf::from("src/sub");
        assert_eq!(app.display_cwd(), "/sub");
        assert!(!app.is_at_root());
        app.cwd = PathBuf::from("src");
        assert!(app.is_at_root());
    }
}
#[cfg(test)]
mod browse_test {
    use super::*;
    use std::fs;

    fn prepare(dir: &Path) {
        if dir.exists() {
            let _ = fs::remove_dir_all(dir);
        }
        fs::create_dir_all(dir).expect("创建测试目录失败 / failed to create test dir");
    }

    #[test]
    fn browse_loads_pack_and_navigates() -> Result<(), Box<dyn std::error::Error>> {
        let base = Path::new(
            "temp/test/command/wbfp/tui/app/browse_test/ok/browse_loads_pack_and_navigates",
        );
        prepare(base);
        let pack_path = base.join("browse.wbfp");

        let src = base.join("src");
        fs::create_dir_all(src.join("sub"))?;
        fs::write(src.join("sub/a.txt"), b"hello")?;
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::command::wbfp::ops::run_pack(&src, &pack_path, true, true, 2, &tx)?;

        let args = WaterBallFilePackCommand::new(WaterBallFilePackCommands::Browse(
            WaterBallFilePackCommandsBrowse {
                pack_path: pack_path.to_string_lossy().into_owned(),
            },
        ));
        let mut app = App::from_args(args)?;
        assert_eq!(app.entries.len(), 1);
        assert_eq!(app.entries[0].name, "sub");
        assert!(app.entries[0].is_dir);

        // 列表操作：Enter 进入子目录，Back 返回 / list operation: Enter enters, Back returns
        app.enter();
        assert_eq!(app.cwd, PathBuf::from("sub"));
        assert_eq!(app.entries.len(), 2);
        assert_eq!(app.entries[0].name, "..");
        assert_eq!(app.entries[1].name, "a.txt");
        assert_eq!(app.entries[1].size, 5);

        app.back();
        assert_eq!(app.cwd, PathBuf::new());
        assert_eq!(app.entries.len(), 1);
        assert_eq!(app.entries[0].name, "sub");

        drop(app);
        fs::remove_dir_all(base)?;
        Ok(())
    }
}

#[cfg(test)]
mod unpack_job_test {
    use super::*;
    use crate::command::wbfp::WaterBallFilePackCommandsUnpack;
    use std::fs;

    fn prepare(dir: &Path) {
        if dir.exists() {
            let _ = fs::remove_dir_all(dir);
        }
        fs::create_dir_all(dir).expect("创建测试目录失败 / failed to create test dir");
    }
    #[test]
    fn unpack_job_shares_pack_handle() -> Result<(), Box<dyn std::error::Error>> {
        let base = Path::new(
            "temp/test/command/wbfp/tui/app/unpack_job_test/ok/unpack_job_shares_pack_handle",
        );
        prepare(base);
        let src = base.join("src");
        let out = base.join("out");
        let pack = base.join("pack.wbfp");
        fs::create_dir_all(src.join("sub"))?;
        fs::write(src.join("a.txt"), b"hello")?;
        fs::write(src.join("sub/b.txt"), b"world")?;

        let (pack_tx, _pack_rx) = std::sync::mpsc::channel();
        crate::command::wbfp::ops::run_pack(&src, &pack, true, true, 2, &pack_tx)?;

        let args = WaterBallFilePackCommand::new(WaterBallFilePackCommands::Unpack(
            WaterBallFilePackCommandsUnpack::new(
                pack.to_string_lossy().into_owned(),
                Some(out.to_string_lossy().into_owned()),
                false,
                Some(2),
            ),
        ));
        let mut app = App::from_args(args)?;
        let spec = app
            .job_spec
            .take()
            .ok_or_else(|| std::io::Error::other("missing unpack job"))?;
        let (tx, rx) = std::sync::mpsc::channel();
        let handle = crate::command::wbfp::ops::spawn(spec, tx)?;
        for event in rx {
            app.apply_event(event);
        }
        handle
            .join()
            .map_err(|_| std::io::Error::other("unpack job panicked"))?;

        assert_eq!(fs::read(out.join("a.txt"))?, b"hello");
        assert_eq!(fs::read(out.join("sub/b.txt"))?, b"world");
        assert_eq!(app.job.as_ref().map(|job| job.finished), Some(true));
        assert_eq!(
            app.statuses.get(&PathBuf::from("a.txt")),
            Some(&RowStatus::Done)
        );
        let sub = app.entries.iter().position(|entry| entry.name == "sub");
        let Some(sub) = sub else {
            panic!("unpacked root is missing the sub directory");
        };
        assert_eq!(app.status_of(&app.entries[sub]), Some(RowStatus::Done));

        drop(app);
        fs::remove_dir_all(base)?;
        Ok(())
    }
}
