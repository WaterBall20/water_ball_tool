//! wbfp TUI 任务事件 / wbfp TUI job events
//!
//! 后台任务线程通过通道把文件发现、进度与结果发送给 UI 线程。
//! 经典进度条路径不经过这里。
//!
//! Background job threads send file discovery, progress and results to the UI
//! thread over a channel. The classic progress-bar path does not use these events.

//AI=== 任务事件与行状态 / job events and row status
use std::path::PathBuf;

/// 文件行状态 / file-row status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RowStatus {
    /// 待处理 / pending
    Pending,
    /// 进行中 / running
    Running,
    /// 完成 / done
    Done,
    /// 警告或跳过 / warning or skipped
    Warning,
    /// 失败 / failed
    Failed,
}

/// 一个后台任务的最终汇总 / final summary of a background job.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct JobSummary {
    pub files_done: u64,
    pub bytes: u64,
    pub failed: u64,
    pub warnings: u64,
}

/// 后台任务发给 UI 的事件 / events sent from the background job to the UI.
#[derive(Debug)]
pub(crate) enum JobEvent {
    /// 发现目录（父目录据此保持未完成直到所有子项结束）/ discovered directory (keeps its parent unfinished)
    Dir { path: PathBuf },
    /// 发现文件 / discovered file
    File { path: PathBuf },
    /// 开始处理一个文件 / started a file
    Start { path: PathBuf, total: u64 },
    /// 文件内进度 / per-file progress
    Progress {
        path: PathBuf,
        done: u64,
        total: u64,
    },
    /// 文件处理结束；`bytes` 用于总进度聚合 / file finished; `bytes` feeds total progress
    Done { path: PathBuf, status: RowStatus },
    /// 文件内容已变更（新建或覆盖；包内相对路径）/ file content changed (created or overwritten; pack-relative path)
    Changed { path: PathBuf },
    /// 全局进度 / overall progress
    Total {
        done: u64,
        total: u64,
        files_done: u64,
        files_total: u64,
    },
    /// 任务结束（成功或失败）/ job finished (success or failure)
    Finished { result: Result<JobSummary, String> },
}
//AI_END===
