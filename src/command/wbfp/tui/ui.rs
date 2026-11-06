//! wbfp TUI 渲染 / wbfp TUI rendering
//!
//! yazi 风格：路径栏、文件列表、预留预览窗、状态行和按键栏。
//!
//! Yazi-inspired layout: path line, file list, reserved preview pane, status line
//! and key hints.

//AI=== yazi 风格渲染 / yazi-inspired rendering
use super::app::{App, JobState, Mode, TaskItem};
use super::browse::{Entry, format_hash, format_modified};
use crate::command::music_play::tui::channel_writer::LogLevel;
use crate::command::wbfp::events::RowStatus;
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Gauge, List, ListItem, Paragraph, Wrap},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
use water_ball_tool_lib::tools::bytes_len_to_string;

const SIZE_NUM_WIDTH: usize = 8;
const SIZE_UNIT_WIDTH: usize = 3;
const SIZE_WIDTH: usize = SIZE_NUM_WIDTH + 1 + SIZE_UNIT_WIDTH;
const TASK_BAR_WIDTH: usize = 6;
const TASK_PERCENT_WIDTH: usize = 4;
const JOB_KEY_WIDTH: usize = 14;
const JOB_DETAIL_HEIGHT: u16 = 5;

pub(super) fn render(app: &mut App, frame: &mut Frame) {
    let [header, path, body, status, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    render_header(app, frame, header);
    render_path(app, frame, path);

    if app.show_preview {
        if frame.area().width >= 80 {
            let [list, preview] =
                Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)])
                    .areas(body);
            render_list(app, frame, list);
            render_preview(app, frame, preview);
        } else {
            render_preview(app, frame, body);
        }
    } else {
        render_list(app, frame, body);
    }
    render_status(app, frame, status);
    render_footer(app, frame, footer);

    if app.show_help {
        render_help(frame);
    }
    if app.show_logs {
        render_logs(app, frame);
    }
}

fn render_header(app: &App, frame: &mut Frame, area: Rect) {
    let mut left = vec![
        Span::styled(
            " wbfp ",
            Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ),
        Span::styled(app.title.clone(), Style::new().fg(Color::White)),
    ];
    if app.mode == Mode::Pack {
        left.push(Span::styled(
            "  ·  只读源目录 / source",
            Style::new().fg(Color::DarkGray),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(left)), area);

    if area.width >= 70 {
        let stats = if let Some(info) = &app.pack_info {
            let cow = if info.cow { " · COW" } else { "" };
            format!(
                "v{} · {} files · {} dirs · {}{cow} ",
                info.version,
                info.file_count,
                info.dir_count,
                bytes_len_to_string(info.data_len)
            )
        } else if let Some(job) = &app.job {
            format!("{} ", job.label)
        } else {
            String::new()
        };
        frame.render_widget(
            Paragraph::new(Span::styled(stats, Style::new().fg(Color::DarkGray)))
                .alignment(Alignment::Right),
            area,
        );
    }
}

fn render_path(app: &App, frame: &mut Frame, area: Rect) {
    let [left, right] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(22)]).areas(area);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" ", Style::new().fg(Color::DarkGray)),
            Span::styled(app.display_cwd(), Style::new().fg(Color::LightCyan)),
        ])),
        left,
    );
    frame.render_widget(
        Paragraph::new(Span::styled(
            "[只读 / READONLY] ",
            Style::new().fg(Color::Yellow),
        ))
        .alignment(Alignment::Right),
        right,
    );
}

fn render_list(app: &mut App, frame: &mut Frame, area: Rect) {
    app.list_area = area;
    app.height = usize::from(area.height.saturating_sub(2)).max(1);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(Color::Cyan))
        .title(Line::from(" 文件 / Files ").style(Style::new().fg(Color::Cyan)));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if app.entries.is_empty() {
        let message = app
            .error
            .clone()
            .unwrap_or_else(|| "空目录 / empty directory".to_string());
        frame.render_widget(
            Paragraph::new(message)
                .style(Style::new().fg(Color::DarkGray))
                .wrap(Wrap { trim: true }),
            inner,
        );
        return;
    }

    let show_size = inner.width >= 56;
    let mut name_max = usize::from(inner.width).saturating_sub(2 + 2);
    if show_size {
        name_max = name_max.saturating_sub(1 + SIZE_WIDTH + 1);
    }
    let name_max = name_max.max(8);
    let items: Vec<ListItem> = app
        .entries
        .iter()
        .enumerate()
        .skip(app.offset)
        .take(app.height)
        .map(|(index, entry)| list_item(app, entry, index == app.selected, show_size, name_max))
        .collect();
    frame.render_widget(List::new(items), inner);
}

fn list_item(
    app: &App,
    entry: &Entry,
    selected: bool,
    show_size: bool,
    name_max: usize,
) -> ListItem<'static> {
    let style = if selected {
        Style::new()
            .bg(Color::Cyan)
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD)
    } else if entry.is_dir {
        Style::new()
            .fg(Color::LightBlue)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new()
    };
    let marker = if entry.synthetic {
        "↑ "
    } else if entry.is_dir {
        "▸ "
    } else {
        "  "
    };
    let name = fit_width(&entry.name, name_max);
    let mut spans = vec![Span::styled(format!(" {marker}{name}"), style)];
    if show_size {
        let size = if entry.is_dir {
            pad_left_width("—", SIZE_WIDTH)
        } else {
            format_size_column(entry.size)
        };
        spans.push(Span::styled(
            format!(" {size}"),
            if selected {
                Style::new().fg(Color::Black)
            } else {
                Style::new().fg(Color::DarkGray)
            },
        ));
    }
    if let Some(status) = app.status_of(entry) {
        let (glyph, color) = status_glyph(status);
        spans.push(Span::styled(
            format!(" {glyph}"),
            if selected {
                Style::new().fg(Color::Black)
            } else {
                Style::new().fg(color)
            },
        ));
    } else {
        spans.push(Span::raw("  "));
    }
    ListItem::new(Line::from(spans)).style(style)
}

fn status_glyph(status: RowStatus) -> (&'static str, Color) {
    match status {
        RowStatus::Pending => ("·", Color::DarkGray),
        RowStatus::Running => ("⟳", Color::Yellow),
        RowStatus::Done => ("✓", Color::Green),
        RowStatus::Warning => ("⚠", Color::Yellow),
        RowStatus::Failed => ("✗", Color::Red),
    }
}

fn render_preview(app: &App, frame: &mut Frame, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(Color::Blue))
        .title(Line::from(" 预览（预留）/ Preview ").style(Style::new().fg(Color::Blue)));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if let Some(job) = &app.job {
        render_job_card(app, job, frame, inner);
        return;
    }

    let mut lines = vec![
        Line::from(Span::styled(
            "预览窗口已预留，后续接入文件内容 / content preview is reserved",
            Style::new().fg(Color::DarkGray),
        )),
        Line::from(""),
    ];
    if let Some(entry) = app.selected_entry() {
        push_metadata(&mut lines, app, entry);
    } else {
        lines.push(Line::from(Span::styled(
            "未选择文件 / no selection",
            Style::new().fg(Color::DarkGray),
        )));
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

fn render_job_card(app: &App, job: &JobState, frame: &mut Frame, area: Rect) {
    let [gauge_area, detail_area] =
        Layout::vertical([Constraint::Length(2), Constraint::Fill(1)]).areas(area);
    let label = if job.error.is_some() {
        "失败 / failed".to_string()
    } else if job.finished {
        "完成 / done".to_string()
    } else if job.total == 0 {
        "扫描中 / discovering".to_string()
    } else {
        format!(
            "{:.0}%  {} / {}",
            job.ratio() * 100.0,
            bytes_len_to_string(job.done),
            bytes_len_to_string(job.total)
        )
    };
    frame.render_widget(
        Gauge::default()
            .gauge_style(Style::new().fg(if job.error.is_some() {
                Color::Red
            } else {
                Color::Cyan
            }))
            .ratio(job.ratio())
            .label(label),
        gauge_area,
    );

    let status_value = if let Some(error) = &job.error {
        Some(Span::styled(error.clone(), Style::new().fg(Color::Red)))
    } else if job.finished {
        Some(Span::styled(
            "完成 / finished",
            Style::new().fg(Color::Green),
        ))
    } else {
        None
    };
    let status_line = match status_value {
        Some(value) => Line::from(vec![Span::raw(" ".repeat(JOB_KEY_WIDTH)), value]),
        None => Line::from(""),
    };
    let lines = vec![
        kv_line("任务 / Task", job.label.clone()),
        kv_line(
            "文件 / Files",
            format!("{:>8} / {:<8}", job.files_done, job.files_total),
        ),
        kv_line(
            "状态 / State",
            format!("⚠ {:>5}   ✗ {:>5}", job.warnings, job.failed),
        ),
        if job.output.is_empty() {
            // 校验没有输出字段：保留一行空白，任务列表位置不跳 / verify has no output: keep the row blank so the task list stays put
            Line::from("")
        } else {
            kv_line("输出 / Output", job.output.clone())
        },
        status_line,
    ];
    let [summary_area, tasks_area] =
        Layout::vertical([Constraint::Length(JOB_DETAIL_HEIGHT), Constraint::Fill(1)])
            .areas(detail_area);
    frame.render_widget(Paragraph::new(lines), summary_area);
    render_tasks(app, frame, tasks_area);
}

fn kv_line(key: &str, value: String) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            fit_width(key, JOB_KEY_WIDTH),
            Style::new().fg(Color::DarkGray),
        ),
        Span::raw(value),
    ])
}

fn render_tasks(app: &App, frame: &mut Frame, area: Rect) {
    if area.height == 0 {
        return;
    }
    let title = Rect::new(area.x, area.y, area.width, 1);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            " 任务 / Tasks",
            Style::new().fg(Color::Cyan),
        ))),
        title,
    );
    if area.height <= 1 {
        return;
    }
    let rows = Rect::new(area.x, area.y + 1, area.width, area.height - 1);
    let mut row = rows.y;
    for task in app.active_tasks.iter().chain(app.done_tasks.iter()) {
        if row >= rows.y.saturating_add(rows.height) {
            break;
        }
        frame.render_widget(
            Paragraph::new(task_line(task, rows.width)),
            Rect::new(rows.x, row, rows.width, 1),
        );
        row += 1;
    }
    if row == rows.y {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "等待文件 / waiting for files",
                Style::new().fg(Color::DarkGray),
            )),
            rows,
        );
    }
}

fn task_line(task: &TaskItem, width: u16) -> Line<'static> {
    let width = usize::from(width);
    let (glyph, color) = status_glyph(task.status);
    let show_percent = width >= 20;
    let show_bar = width >= 30;
    let show_sizes = width >= 52;
    let fixed = 3
        + if show_bar { 9 } else { 0 }
        + if show_percent { 5 } else { 0 }
        + if show_sizes {
            1 + SIZE_WIDTH + 1 + SIZE_WIDTH
        } else {
            0
        };
    let name_width = width.saturating_sub(fixed).max(8);
    let name = fit_width(&task.name, name_width);
    let mut spans = vec![
        Span::styled(format!(" {glyph} "), Style::new().fg(color)),
        Span::raw(name),
    ];

    let ratio = task.ratio();
    if show_bar {
        let filled = (ratio * TASK_BAR_WIDTH as f64).round() as usize;
        let bar = format!(
            "[{}{}]",
            "█".repeat(filled),
            "░".repeat(TASK_BAR_WIDTH - filled)
        );
        spans.push(Span::styled(
            format!(" {bar}"),
            Style::new().fg(match task.status {
                RowStatus::Done => Color::Green,
                RowStatus::Failed => Color::Red,
                _ => Color::Yellow,
            }),
        ));
    }
    if show_percent {
        let percent = if task.total == 0 {
            "—".to_string()
        } else {
            format!("{:>3.0}%", ratio * 100.0)
        };
        spans.push(Span::styled(
            format!(" {}", pad_left_width(&percent, TASK_PERCENT_WIDTH)),
            Style::new().fg(match task.status {
                RowStatus::Done => Color::Green,
                RowStatus::Failed => Color::Red,
                _ => Color::Yellow,
            }),
        ));
    }

    if show_sizes {
        let done = format_size_column(task.done);
        let total = if task.total == 0 {
            pad_left_width("—", SIZE_WIDTH)
        } else {
            format_size_column(task.total)
        };
        spans.push(Span::styled(
            format!(" {done}/{total}"),
            Style::new().fg(Color::DarkGray),
        ));
    }

    Line::from(spans)
}

fn push_metadata(lines: &mut Vec<Line<'static>>, app: &App, entry: &Entry) {
    lines.push(Line::from(vec![
        Span::styled("name      ", Style::new().fg(Color::DarkGray)),
        Span::raw(entry.name.clone()),
    ]));
    lines.push(Line::from(vec![
        Span::styled("type      ", Style::new().fg(Color::DarkGray)),
        Span::raw(if entry.is_dir { "dir" } else { "file" }),
    ]));
    lines.push(Line::from(vec![
        Span::styled("size      ", Style::new().fg(Color::DarkGray)),
        Span::raw(bytes_len_to_string(entry.size)),
    ]));
    lines.push(Line::from(vec![
        Span::styled("modified  ", Style::new().fg(Color::DarkGray)),
        Span::raw(format_modified(entry.modified)),
    ]));
    lines.push(Line::from(vec![
        Span::styled("hash      ", Style::new().fg(Color::DarkGray)),
        Span::raw(if app.mode == Mode::Pack {
            "—（打包后生成 / generated after packing）".to_string()
        } else {
            format_hash(entry.hash.as_ref())
        }),
    ]));
    if let Some(status) = app.status_of(entry) {
        let (glyph, color) = status_glyph(status);
        lines.push(Line::from(vec![
            Span::styled("status    ", Style::new().fg(Color::DarkGray)),
            Span::styled(glyph, Style::new().fg(color)),
        ]));
    }
}

fn render_status(app: &App, frame: &mut Frame, area: Rect) {
    let (ok, warn, fail) = app
        .job
        .as_ref()
        .map(|job| {
            (
                job.files_done.saturating_sub(job.failed),
                job.warnings,
                job.failed,
            )
        })
        .unwrap_or((0, 0, 0));
    let position = if app.entries.is_empty() {
        0
    } else {
        app.selected + 1
    };
    let left = format!(
        " {position:>4}/{:<4} 项 · 已选 {:>10} · ✓ {:>5}  ⚠ {:>5}  ✗ {:>5}",
        app.entries.len(),
        pad_left_width(&format_size_column(app.selected_size()), SIZE_WIDTH),
        pad_left_width(&ok.to_string(), 5),
        pad_left_width(&warn.to_string(), 5),
        pad_left_width(&fail.to_string(), 5),
    );
    let right = if let Some(job) = &app.job {
        if job.error.is_some() {
            "失败 / failed"
        } else if job.finished {
            "完成 / done"
        } else if job.files_total == 0 && job.total == 0 {
            "扫描中 / discovering"
        } else {
            "运行中 / running"
        }
    } else {
        "就绪 / ready"
    };
    let [l, r] = Layout::horizontal([Constraint::Fill(1), Constraint::Length(22)]).areas(area);
    frame.render_widget(Paragraph::new(left).style(Style::new().fg(Color::Gray)), l);
    frame.render_widget(
        Paragraph::new(right)
            .alignment(Alignment::Right)
            .style(Style::new().fg(if app.job_running {
                Color::Yellow
            } else {
                Color::DarkGray
            })),
        r,
    );
}

fn render_footer(app: &App, frame: &mut Frame, area: Rect) {
    let text = if app.job_running {
        " ↑↓ 滚动   d 日志   ? 帮助   q 完成后退出 "
    } else {
        " q 退出   ↑↓ 移动   ⏎ 进入   ← 上级   p 预览   d 日志   ? 帮助 "
    };
    frame.render_widget(
        Paragraph::new(Line::from(text))
            .alignment(Alignment::Center)
            .style(Style::new().fg(Color::DarkGray)),
        area,
    );
}

fn render_help(frame: &mut Frame) {
    let area = centered(frame.area(), 72, 62);
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(Color::Cyan))
        .title(Line::from(" 帮助 / Help ").style(Style::new().fg(Color::Cyan)));
    let lines = vec![
        Line::from("q / Ctrl+C   退出 / quit（任务运行中会等完成）"),
        Line::from("Esc          返回上级 / parent directory"),
        Line::from("↑↓ / j k     移动 / move"),
        Line::from("PgUp PgDn    翻页 / page"),
        Line::from("Home End     首尾 / first last"),
        Line::from("⏎ / → / l    进入目录 / enter directory"),
        Line::from("左键 / 双击  选择 / 进入 / select / open（非 Windows / non-Windows）"),
        Line::from("Windows      鼠标暂禁用，仅键盘 / mouse disabled, keyboard only"),
        Line::from("← / h        上级 / parent"),
        Line::from("p            切换预览 / toggle preview"),
        Line::from("d / F12      日志 / logs"),
        Line::from("?            帮助 / help"),
        Line::from(""),
        Line::from(Span::styled(
            "文件列表在打包/解包/校验时仅支持预览，不提供编辑操作。",
            Style::new().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            "The file list is read-only during pack/unpack/verify.",
            Style::new().fg(Color::DarkGray),
        )),
    ];
    frame.render_widget(
        Paragraph::new(lines).block(block).wrap(Wrap { trim: true }),
        area,
    );
}

fn render_logs(app: &App, frame: &mut Frame) {
    let area = centered(frame.area(), 90, 70);
    frame.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(Color::Yellow))
        .title(Line::from(" 日志 / Logs ").style(Style::new().fg(Color::Yellow)));
    let mut lines = Vec::new();
    let count = usize::from(area.height.saturating_sub(2)).max(1);
    for (level, text) in app.last_logs(count) {
        let color = match level {
            Some(LogLevel::Error) => Color::Red,
            Some(LogLevel::Warn) => Color::Yellow,
            Some(LogLevel::Info) => Color::Gray,
            Some(LogLevel::Debug | LogLevel::Trace) => Color::DarkGray,
            None => Color::Gray,
        };
        lines.push(Line::from(Span::styled(
            text.to_string(),
            Style::new().fg(color),
        )));
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "暂无日志 / no logs",
            Style::new().fg(Color::DarkGray),
        )));
    }
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn centered(area: Rect, percent_x: u16, percent_y: u16) -> Rect {
    let width = area.width.saturating_mul(percent_x) / 100;
    let height = area.height.saturating_mul(percent_y) / 100;
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

fn display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

fn truncate_width(text: &str, max: usize) -> String {
    if display_width(text) <= max {
        return text.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + width > max.saturating_sub(1) {
            break;
        }
        out.push(ch);
        used += width;
    }
    out.push('…');
    out
}

fn fit_width(text: &str, width: usize) -> String {
    let mut out = truncate_width(text, width);
    out.push_str(&" ".repeat(width.saturating_sub(display_width(&out))));
    out
}

fn pad_left_width(text: &str, width: usize) -> String {
    let mut out = " ".repeat(width.saturating_sub(display_width(text)));
    out.push_str(text);
    out
}

/// 数字与单位分成固定两列，避免 `B`/`KiB` 单位宽度不同导致数字横向跳动。
///
/// Split the number and unit into fixed columns so the one-cell `B` unit cannot shift the number.
fn format_size_column(bytes: u64) -> String {
    let text = bytes_len_to_string(bytes);
    let split = text
        .find(|ch: char| !ch.is_ascii_digit() && ch != '.')
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    format!(
        "{} {}",
        pad_left_width(number, SIZE_NUM_WIDTH),
        fit_width(unit, SIZE_UNIT_WIDTH)
    )
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn size_column_keeps_number_and_unit_aligned() {
        for bytes in [0, 180, 12_684, 1_234_567_890, 18_386_554_201] {
            let column = format_size_column(bytes);
            assert_eq!(display_width(&column), SIZE_WIDTH);
        }
        assert!(format_size_column(180).ends_with("B  "));
        assert!(format_size_column(12_684).ends_with("KiB"));
    }
}
//AI_END===
