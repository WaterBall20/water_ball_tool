//AI=== 命令帮助弹窗复用命令表 / the help popup reuses the command table
use crate::command::music_play::tui::COMMANDS;
use crate::command::music_play::tui::app::{App, FooterAction};
use crate::command::music_play::tui::channel_writer::{LogLevel, LogLine};
//AI_END===
//AI=== 播放列表滚动需要 Rect / playlist scrolling needs Rect
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Gauge, List, ListItem, Paragraph},
};
//AI_END===
use std::time::Duration;

//AI=== TUI 静态布局预览：正在播放 / 进度 / 播放列表 / 日志 / 快捷键
// Static TUI layout preview: now playing / progress / playlist / logs / key hints.
pub fn render(app: &mut App, frame: &mut Frame) {
    let [now, progress, body, footer] = Layout::vertical([
        Constraint::Length(8),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    render_now_playing(app, frame, now);
    render_progress(app, frame, progress);

    let [playlist, logs] =
        Layout::horizontal([Constraint::Percentage(42), Constraint::Percentage(58)]).areas(body);
    render_playlist(app, frame, playlist);
    render_logs(app, frame, logs);

    //AI=== 底栏：命令输入、错误提示或快捷键 / footer: command input, error notice or key hints
    render_footer(app, frame, footer);
    //AI_END===

    if app.show_debug {
        render_debug(app, frame);
    }
    //AI=== 帮助弹窗显示在最上层 / the help popup renders on top
    if app.show_help {
        render_help(frame);
    }
    //AI_END===
}

//AI=== 底栏提示项：文字与可点击动作 / footer hints: label and clickable action
const FOOTER_HINTS: &[(FooterAction, &str)] = &[
    (FooterAction::Quit, "q 退出"),
    (FooterAction::Next, "n 下一曲"),
    (FooterAction::SeekBack, "← 快退"),
    (FooterAction::SeekForward, "→ 快进"),
    (FooterAction::Toggle, "空格 播放/暂停"),
    (FooterAction::ScrollUp, "↑ 上滚"),
    (FooterAction::ScrollDown, "↓ 下滚"),
    (FooterAction::VolumeDown, "− 音量"),
    (FooterAction::VolumeUp, "+ 音量"),
    (FooterAction::Debug, "d 调试"),
    (FooterAction::Command, ": 命令"),
];
const FOOTER_GAP: &str = "  ";
//AI_END===

//AI=== 底栏渲染：`:<输入>` 左对齐，其余居中；快捷键文字本身可点击
// Footer: `:<input>` is left-aligned, everything else centered; the hint labels are clickable.
fn render_footer(app: &mut App, frame: &mut Frame, area: Rect) {
    app.footer_area = area;
    app.footer_actions.clear();
    let (line, alignment) = if let Some(input) = &app.command {
        (
            Line::from(vec![
                Span::styled(
                    " :",
                    Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                ),
                Span::styled(input.clone(), Style::new().fg(Color::White)),
                Span::styled("▏", Style::new().fg(Color::Cyan)),
            ]),
            Alignment::Left,
        )
    } else if let Some(message) = &app.command_error {
        (
            Line::from(Span::styled(
                format!(" {message} "),
                Style::new().fg(Color::Red),
            )),
            Alignment::Center,
        )
    } else {
        let mut spans = Vec::with_capacity(FOOTER_HINTS.len() * 2);
        let mut total = 0usize;
        for (position, (_, label)) in FOOTER_HINTS.iter().enumerate() {
            if position > 0 {
                spans.push(Span::raw(FOOTER_GAP));
                total += Span::raw(FOOTER_GAP).width();
            }
            let span = Span::styled(*label, Style::new().fg(Color::DarkGray));
            total += span.width();
            spans.push(span);
        }

        let start = area
            .x
            .saturating_add(area.width.saturating_sub(u16::try_from(total).unwrap_or(0)) / 2);
        let mut column = start;
        for (position, (action, label)) in FOOTER_HINTS.iter().enumerate() {
            if position > 0 {
                column = column.saturating_add(Span::raw(FOOTER_GAP).width() as u16);
            }
            let width = Span::raw(*label).width() as u16;
            app.footer_actions
                .push((column, column.saturating_add(width), *action));
            column = column.saturating_add(width);
        }

        (Line::from(spans), Alignment::Center)
    };

    frame.render_widget(
        Paragraph::new(line)
            .alignment(alignment)
            .style(Style::new().fg(Color::DarkGray)),
        area,
    );
}

/// 命令帮助弹窗：直接渲染命令表 / command help popup: renders the command table
fn render_help(frame: &mut Frame) {
    let [_, middle, _] = Layout::vertical([
        Constraint::Percentage(10),
        Constraint::Percentage(80),
        Constraint::Percentage(10),
    ])
    .areas(frame.area());
    let [_, popup, _] = Layout::horizontal([
        Constraint::Percentage(5),
        Constraint::Percentage(90),
        Constraint::Percentage(5),
    ])
    .areas(middle);

    frame.render_widget(Clear, popup);

    let mut lines = vec![
        Line::from(Span::styled(
            " 输入 : 前缀；命令可只写唯一前缀，例如 :pl 等于 :play",
            Style::new().fg(Color::Gray),
        )),
        Line::from(""),
    ];
    lines.extend(COMMANDS.iter().map(|(_, usage, description)| {
        Line::from(vec![
            Span::styled(format!("  :{usage:<12}"), Style::new().fg(Color::Yellow)),
            Span::styled(*description, Style::new().fg(Color::Gray)),
        ])
    }));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Esc 关闭帮助",
        Style::new().fg(Color::DarkGray),
    )));

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(Color::Cyan))
        .title(
            Line::from(" 命令帮助 ")
                .style(Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        );
    frame.render_widget(Paragraph::new(lines).block(block), popup);
}
//AI_END===

fn render_now_playing(app: &App, frame: &mut Frame, area: Rect) {
    let (state, state_color) = if app.poisoned {
        ("异常暂停中", Color::Red)
    } else if app.playing {
        ("播放中", Color::Green)
    } else {
        ("暂停中", Color::Yellow)
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(Color::Cyan))
        .title(
            Line::from(" 正在播放 ")
                .style(Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        )
        .title(
            Line::from(format!(
                " {}  ·  音量 {:.0}%  ·  {} ",
                app.mode,
                app.volume * 100.0,
                state
            ))
            .right_aligned()
            .style(Style::new().fg(state_color)),
        );

    let mut lines = vec![
        Line::from(Span::styled(
            if app.title.is_empty() {
                "等待曲目… / waiting for a track".to_string()
            } else {
                app.title.clone()
            },
            Style::new().fg(Color::White).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            app.artist.clone(),
            Style::new().fg(Color::LightBlue),
        )),
        Line::from(Span::styled(
            app.album.clone(),
            Style::new().fg(Color::Gray),
        )),
        Line::from(Span::styled(
            app.audio_info.clone(),
            Style::new().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            app.path.clone(),
            Style::new().fg(Color::DarkGray),
        )),
    ];

    if app.poisoned {
        lines.push(Line::from(vec![
            Span::styled(
                "Error",
                Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::styled(": 解码错误，按空格换曲恢复", Style::new().fg(Color::Red)),
        ]));
    }

    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_progress(app: &App, frame: &mut Frame, area: Rect) {
    //AI=== 长按预览优先显示；预览时标注“跳转预览” / the hold preview wins and is labelled
    let pos = app.display_pos();
    let preview = app.seek_gesture.is_some();
    let ratio = if app.duration.is_zero() {
        0.0
    } else {
        (pos.as_secs_f64() / app.duration.as_secs_f64()).clamp(0.0, 1.0)
    };

    frame.render_widget(
        Gauge::default()
            .gauge_style(Style::new().fg(if preview { Color::Yellow } else { Color::Cyan }))
            .ratio(ratio)
            .label(Span::styled(
                format!(
                    " {} / {}  ·  {:.0}%{} ",
                    fmt_time(pos),
                    fmt_time(app.duration),
                    ratio * 100.0,
                    if preview { "  ·  跳转预览" } else { "" }
                ),
                Style::new().fg(Color::White),
            )),
        area,
    );
    //AI_END===
}

//AI=== 播放列表视口：只渲染可见行，记录区域供鼠标命中 / playlist viewport: render visible rows only, record the area for mouse hit-testing
fn render_playlist(app: &mut App, frame: &mut Frame, area: Rect) {
    let visible = usize::from(area.height.saturating_sub(2));
    app.playlist_area = area;
    app.playlist_height = visible;
    // 列表缩短或窗口变高后把偏移夹回有效范围 / clamp the offset after a shrink or a taller terminal
    app.scroll_playlist_to(app.playlist_offset);

    let items: Vec<ListItem> = app
        .playlist
        .iter()
        .enumerate()
        .skip(app.playlist_offset)
        .take(visible)
        .map(|(i, track)| {
            let selected = app.current == Some(i);
            let marker = if selected { "▶ " } else { "  " };
            let style = if selected {
                Style::new()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::new().fg(Color::Gray)
            };
            ListItem::new(Line::from(Span::styled(
                format!("{marker}{:>2}. {track}", i + 1),
                style,
            )))
        })
        .collect();

    let mut title = if app.scan_done {
        format!(" 播放列表 ({})", app.playlist.len())
    } else {
        format!(
            " 播放列表 ({}) · 发现 {}/{}",
            app.playlist.len(),
            app.discovery.0,
            app.discovery.1
        )
    };
    if app.playlist.len() > visible && visible > 0 {
        title.push_str(&format!(
            " · {}-{}/{}",
            app.playlist_offset + 1,
            (app.playlist_offset + visible).min(app.playlist.len()),
            app.playlist.len()
        ));
    }
    title.push(' ');
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(Color::Cyan))
        .title(Line::from(title).style(Style::new().fg(Color::Cyan)));

    frame.render_widget(List::new(items).block(block), area);
}
//AI_END===

fn render_logs(app: &App, frame: &mut Frame, area: Rect) {
    // 只渲染能放下的最后若干行，避免 1000 行缓存从最早的行开始显示
    // Render only the last lines that fit, so the 1000-line buffer does not start from the oldest entry.
    let visible = usize::from(area.height.saturating_sub(2));
    let skip = app.logs.len().saturating_sub(visible);
    let lines: Vec<Line> = app.logs.iter().skip(skip).map(log_line).collect();

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(Color::Cyan))
        .title(Line::from(" 日志 ").style(Style::new().fg(Color::Cyan)));

    frame.render_widget(Paragraph::new(lines).block(block), area);
}

//AI=== Debug 弹窗：缓冲区、解码器状态与各段耗时 / debug popup: buffers, decoder state and per-stage timings
fn render_debug(app: &App, frame: &mut Frame) {
    let [_, middle, _] = Layout::vertical([
        Constraint::Percentage(5),
        Constraint::Percentage(90),
        Constraint::Percentage(5),
    ])
    .areas(frame.area());
    let [_, popup, _] = Layout::horizontal([
        Constraint::Percentage(5),
        Constraint::Percentage(90),
        Constraint::Percentage(5),
    ])
    .areas(middle);

    frame.render_widget(Clear, popup);

    let debug = &app.debug;
    let format = debug.decoder_format.map_or_else(
        || "—".to_string(),
        |(rate, channels)| format!("{rate} Hz / {channels} ch"),
    );
    let state = if app.poisoned {
        "异常暂停中 / poisoned"
    } else if app.playing {
        "播放中 / playing"
    } else {
        "暂停中 / paused"
    };
    let current = debug
        .playlist_index
        .map_or_else(|| "—".to_string(), |index| (index + 1).to_string());

    let mut lines = vec![
        debug_kv(
            "解码歌曲",
            debug
                .decoder_path
                .clone()
                .unwrap_or_else(|| "—".to_string()),
        ),
        debug_kv("解码格式", format),
        debug_kv(
            "缓冲区",
            format!(
                "pre {} / {}  ·  audio {} samples",
                debug.pre_effect_len, debug.pre_effect_capacity, debug.audio_len
            ),
        ),
        debug_kv(
            "播放列表",
            format!("{} 首 · 当前 {}", debug.playlist_len, current),
        ),
        debug_kv(
            "进度",
            format!("{} / {}", fmt_time(app.pos), fmt_time(app.duration)),
        ),
        debug_kv("音量", format!("{:.0}%", app.volume * 100.0)),
        debug_kv("状态", format!("{state} · eof {}", debug.pre_effect_eof)),
        debug_kv(
            "错误计数",
            format!(
                "lock_fail {} · underrun {}",
                debug.lock_fail, debug.underrun
            ),
        ),
        debug_kv(
            "扫描/日志",
            format!(
                "发现 {} 文件 {} 目录 · 日志 {} 行",
                app.discovery.0,
                app.discovery.1,
                app.logs.len()
            ),
        ),
        Line::from(""),
        Line::from(Span::styled(
            "各段耗时（窗口最大 / 平均，ms）",
            Style::new().fg(Color::Green),
        )),
    ];

    for (name, window) in &debug.metrics {
        let max_ms = window.max_ns as f64 / 1_000_000.0;
        let avg_ms = window.mean().as_secs_f64() * 1000.0;
        lines.push(Line::from(format!(
            "  {name:<20} max {max_ms:>8.3}   avg {avg_ms:>8.3}"
        )));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(Color::Cyan))
        .title(
            Line::from(" Debug · d/F12 关闭 ")
                .style(Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        );
    frame.render_widget(Paragraph::new(lines).block(block), popup);
}

fn debug_kv(label: &'static str, value: String) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label:<10}"), Style::new().fg(Color::Green)),
        Span::styled(value, Style::new().fg(Color::Gray)),
    ])
}
//AI_END===

fn fmt_time(d: Duration) -> String {
    let secs = d.as_secs();
    format!("{:02}:{:02}", secs / 60, secs % 60)
}

//AI=== 日志分色：时间戳暗灰、级别按严重度着色、消息灰色
// Log coloring: dim timestamp, level color by severity, gray message.
fn log_line(line: &LogLine) -> Line<'static> {
    let Some(level) = line.level else {
        return Line::from(Span::styled(
            line.text.clone(),
            Style::new().fg(Color::Gray),
        ));
    };
    let token = level.token();
    let Some(pos) = line.text.find(token) else {
        return Line::from(Span::styled(
            line.text.clone(),
            Style::new().fg(Color::Gray),
        ));
    };

    let head = line.text[..pos].to_string();
    let tail = line.text[pos + token.len()..].to_string();
    Line::from(vec![
        Span::styled(head, Style::new().fg(Color::DarkGray)),
        Span::styled(
            token.to_string(),
            Style::new()
                .fg(level_color(level))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(tail, Style::new().fg(Color::Gray)),
    ])
}

fn level_color(level: LogLevel) -> Color {
    match level {
        LogLevel::Trace => Color::DarkGray,
        LogLevel::Debug => Color::Blue,
        LogLevel::Info => Color::Green,
        LogLevel::Warn => Color::Yellow,
        LogLevel::Error => Color::Red,
    }
}
//AI_END===
