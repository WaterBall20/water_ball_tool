//! wbfp TUI 渲染 / wbfp TUI rendering
//!
//! yazi 风格：路径栏、文件列表、预留预览窗、状态行和按键栏。
//!
//! Yazi-inspired layout: path line, file list, reserved preview pane, status line
//! and key hints.

//AI=== yazi 风格渲染 / yazi-inspired rendering
use super::app::{App, Focus, JobState, Mode, Nav, TaskItem, Window};
use super::browse::{Entry, format_hash, format_modified};
use crate::command::music_play::tui::channel_writer::LogLevel;
use crate::command::wbfp::events::RowStatus;
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
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
//AI=== Manager 三栏/两栏折叠阈值 / manager three-pane and two-pane fold thresholds
const MANAGER_THREE_PANE_MIN_WIDTH: u16 = 110;
const MANAGER_TWO_PANE_MIN_WIDTH: u16 = 80;
//AI_END===

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
    if app.ui.mode == Mode::Manager {
        render_manager(app, frame, body);
    } else {
        render_single(app, frame, body);
    }
    render_status(app, frame, status);
    render_footer(app, frame, footer);

    match app.ui.window {
        Window::Help => render_help(frame),
        Window::Debug => render_logs(app, frame),
        Window::None => {}
    }
}

/// 子命令模式的原有单栏布局 / the existing single-pane layout for subcommands.
fn render_single(app: &mut App, frame: &mut Frame, body: Rect) {
    if app.nav.show_preview {
        if frame.area().width >= 80 {
            let [list, preview] =
                Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)])
                    .areas(body);
            prepare_nav(&mut app.nav, list);
            render_list(app, &app.nav, frame, list, " 文件 / Files ", true);
            render_preview(app, frame, preview);
        } else {
            render_preview(app, frame, body);
        }
    } else {
        prepare_nav(&mut app.nav, body);
        render_list(app, &app.nav, frame, body, " 文件 / Files ", true);
    }
}

fn prepare_nav(nav: &mut Nav, area: Rect) {
    nav.list_area = area;
    nav.height = usize::from(area.height.saturating_sub(2)).max(1);
}

/// Manager 模式：左文件系统、中包、右预览；先折叠预览，再折叠文件系统。
/// Manager: filesystem left, pack middle, preview right; preview folds first, then filesystem.
fn render_manager(app: &mut App, frame: &mut Frame, body: Rect) {
    let has_pack = app.pack.is_some();
    let want_preview = app.nav.show_preview;
    let width = body.width;

    // 焦点不能停在被折叠的面板上 / focus must not stay on a folded pane
    if !has_pack {
        app.focus = Focus::Local;
    } else if width < MANAGER_TWO_PANE_MIN_WIDTH && app.focus == Focus::Local {
        app.focus = Focus::Pack;
    }

    if has_pack && width >= MANAGER_THREE_PANE_MIN_WIDTH && want_preview {
        let [local, pack, preview] = Layout::horizontal([
            Constraint::Percentage(30),
            Constraint::Percentage(40),
            Constraint::Percentage(30),
        ])
        .areas(body);
        prepare_nav(&mut app.local_nav, local);
        prepare_nav(&mut app.nav, pack);
        render_list(
            app,
            &app.local_nav,
            frame,
            local,
            " 文件系统 / Filesystem ",
            app.focus == Focus::Local,
        );
        render_list(
            app,
            &app.nav,
            frame,
            pack,
            " 包 / Pack ",
            app.focus == Focus::Pack,
        );
        render_preview(app, frame, preview);
    } else if has_pack && width >= MANAGER_TWO_PANE_MIN_WIDTH {
        let [local, pack] =
            Layout::horizontal([Constraint::Percentage(35), Constraint::Percentage(65)])
                .areas(body);
        prepare_nav(&mut app.local_nav, local);
        prepare_nav(&mut app.nav, pack);
        render_list(
            app,
            &app.local_nav,
            frame,
            local,
            " 文件系统 / Filesystem ",
            app.focus == Focus::Local,
        );
        render_list(
            app,
            &app.nav,
            frame,
            pack,
            " 包 / Pack ",
            app.focus == Focus::Pack,
        );
    } else if has_pack {
        prepare_nav(&mut app.nav, body);
        render_list(app, &app.nav, frame, body, " 包 / Pack ", true);
    } else if want_preview && width >= MANAGER_TWO_PANE_MIN_WIDTH {
        let [local, preview] =
            Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)])
                .areas(body);
        prepare_nav(&mut app.local_nav, local);
        render_list(
            app,
            &app.local_nav,
            frame,
            local,
            " 文件系统 / Filesystem ",
            true,
        );
        render_preview(app, frame, preview);
    } else {
        prepare_nav(&mut app.local_nav, body);
        render_list(
            app,
            &app.local_nav,
            frame,
            body,
            " 文件系统 / Filesystem ",
            true,
        );
    }
}

fn render_header(app: &App, frame: &mut Frame, area: Rect) {
    let mut left = vec![
        Span::styled(
            " wbfp ",
            Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ),
        Span::styled(app.ui.title.clone(), Style::new().fg(Color::White)),
    ];
    if app.ui.mode == Mode::Pack {
        left.push(Span::styled(
            "  ·  只读源目录 / source",
            Style::new().fg(Color::DarkGray),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(left)), area);

    if area.width >= 70 {
        let stats = if let Some(info) = &app.ui.pack_info {
            let cow = if info.cow { " · COW" } else { "" };
            format!(
                "v{} · {} files · {} dirs · {}{cow} ",
                info.version,
                info.file_count,
                info.dir_count,
                bytes_len_to_string(info.data_len)
            )
        } else if let Some(job) = &app.jobs.state {
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
    let (tag, tag_color) = if app.ui.mode == Mode::Manager {
        match app.focus {
            Focus::Local => ("[文件系统 / FS] ", Color::Cyan),
            Focus::Pack => ("[包 / PACK] ", Color::Magenta),
        }
    } else {
        ("", Color::DarkGray)
    };
    let right_text = if app.ui.mode == Mode::Manager {
        if app.pack.is_some() {
            "[读写 / RW] "
        } else {
            "[无包 / NO PACK] "
        }
    } else {
        "[只读 / READONLY] "
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" ", Style::new().fg(Color::DarkGray)),
            Span::styled(tag, Style::new().fg(tag_color)),
            Span::styled(app.display_cwd(), Style::new().fg(Color::LightCyan)),
        ])),
        left,
    );
    frame.render_widget(
        Paragraph::new(Span::styled(
            right_text,
            Style::new().fg(if app.pack.is_some() || app.ui.mode != Mode::Manager {
                Color::Yellow
            } else {
                Color::DarkGray
            }),
        ))
        .alignment(Alignment::Right),
        right,
    );
}

fn render_list(app: &App, nav: &Nav, frame: &mut Frame, area: Rect, title: &str, focused: bool) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(if focused {
            Color::Cyan
        } else {
            Color::DarkGray
        }))
        .title(Line::from(title).style(Style::new().fg(if focused {
            Color::Cyan
        } else {
            Color::Gray
        })));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if nav.entries.is_empty() {
        let message = app
            .ui
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
    let items: Vec<ListItem> = nav
        .entries
        .iter()
        .enumerate()
        .skip(nav.offset)
        .take(nav.height)
        .map(|(index, entry)| list_item(app, entry, index == nav.selected, show_size, name_max))
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
    } else if entry.pack_version.is_some() {
        //AI=== 探测到的水球包使用独立颜色 / probed WaterBall packs get a distinct color
        Style::new().fg(Color::Magenta).add_modifier(Modifier::BOLD)
        //AI_END===
    } else {
        Style::new()
    };
    let marker = if entry.synthetic {
        "↑ "
    } else if entry.is_dir {
        "▸ "
    } else if entry.pack_version.is_some() {
        //AI=== 包文件图标 / pack-file icon
        "◆ "
        //AI_END===
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

    if let Some(job) = &app.jobs.state {
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
    // 进度条独占第一行，标签放第二行，避免文字把进度条挖空。
    // The bar owns the first row and the label the second, so text never punches a hole in the bar.
    let [bar_area, label_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(gauge_area);
    //AI=== 与 mp 共用的八分块进度条 / shared eighth-block bar with mp
    crate::command::render_eighth_block_bar(
        frame,
        bar_area,
        job.ratio(),
        if job.error.is_some() {
            Color::Red
        } else {
            Color::Cyan
        },
        "",
        Style::new(),
    );
    //AI_END===
    frame.render_widget(
        Paragraph::new(Span::styled(label, Style::new())).alignment(Alignment::Center),
        label_area,
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
    for task in app.jobs.active.iter().chain(app.jobs.done.iter()) {
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
        //AI=== 与 mp 共用的八分块进度条 / shared eighth-block bar with mp
        spans.push(Span::styled(
            format!(
                " [{}]",
                crate::command::eighth_block_bar(TASK_BAR_WIDTH, ratio)
            ),
            Style::new().fg(match task.status {
                RowStatus::Done => Color::Green,
                RowStatus::Failed => Color::Red,
                _ => Color::Yellow,
            }),
        ));
        //AI_END===
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
        Span::raw(if let Some(version) = entry.pack_version {
            format!("WBFilesPack v{version}")
        } else if entry.is_dir {
            "dir".to_string()
        } else {
            "file".to_string()
        }),
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
        Span::raw(if app.ui.mode == Mode::Pack {
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
        .jobs
        .state
        .as_ref()
        .map(|job| {
            (
                job.files_done.saturating_sub(job.failed),
                job.warnings,
                job.failed,
            )
        })
        .unwrap_or((0, 0, 0));
    let nav = app.active_nav();
    let position = if nav.entries.is_empty() {
        0
    } else {
        nav.selected + 1
    };
    let left = format!(
        " {position:>4}/{:<4} 项 · 已选 {:>10} · ✓ {:>5}  ⚠ {:>5}  ✗ {:>5}",
        nav.entries.len(),
        pad_left_width(&format_size_column(app.selected_size()), SIZE_WIDTH),
        pad_left_width(&ok.to_string(), 5),
        pad_left_width(&warn.to_string(), 5),
        pad_left_width(&fail.to_string(), 5),
    );
    let right = if let Some(job) = &app.jobs.state {
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
            .style(Style::new().fg(if app.jobs.running {
                Color::Yellow
            } else {
                Color::DarkGray
            })),
        r,
    );
}

fn render_footer(app: &App, frame: &mut Frame, area: Rect) {
    let text = if app.jobs.running {
        " ↑↓ 滚动   F12 调试   ? 帮助   q 完成后退出 "
    } else if app.ui.mode == Mode::Manager {
        if app.pack.is_some() {
            " Tab 切换面板   ⏎ 进入/打开   c 复制   m 移动   Ctrl+W 关闭包   p 预览   F12 调试   ? 帮助   q 退出 "
        } else {
            " ⏎ 打开包/进入目录   ↑↓ 移动   ← 上级   p 预览   F12 调试   ? 帮助   q 退出 "
        }
    } else {
        " q 退出   ↑↓ 移动   ⏎ 进入   ← 上级   p 预览   F12 调试   ? 帮助 "
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
        Line::from("Esc          关闭弹层 / 返回上级 / close popup / parent"),
        Line::from("Tab          切换文件系统与包面板 / switch panes"),
        Line::from("↑↓ / j k     移动 / move"),
        Line::from("PgUp PgDn    翻页 / page"),
        Line::from("Home End     首尾 / first last"),
        Line::from("⏎ / → / l    进入目录；本地包文件打开 / enter; open a local pack"),
        Line::from("c / m        复制 / 移动到另一面板 / copy / move to the other pane"),
        Line::from("Ctrl+W       关闭当前包 / close the open pack"),
        Line::from("左键 / 双击  选择 / 进入 / select / open"),
        Line::from("← / h        上级 / parent"),
        Line::from("p            切换预览 / toggle preview"),
        Line::from("F12          调试 / debug"),
        Line::from("?            帮助 / help"),
        Line::from(""),
        Line::from(Span::styled(
            "左侧文件按文件头格式探测水球包，不依赖扩展名。",
            Style::new().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            "Local files are probed by header format, not by extension.",
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
        .title(Line::from(" 调试 / Debug ").style(Style::new().fg(Color::Yellow)));
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
mod test;
//AI_END===
