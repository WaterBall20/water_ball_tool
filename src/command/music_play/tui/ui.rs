//AI=== 命令帮助弹窗复用命令表 / the help popup reuses the command table
use crate::command::music_play::tui::COMMANDS;
use crate::command::music_play::tui::app::{App, FooterAction, Window};
use crate::command::music_play::tui::channel_writer::{LogLevel, LogLine};
//AI_END===
//AI=== 播放列表滚动需要 Rect / playlist scrolling needs Rect
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Gauge, List, ListItem, ListState, Paragraph, Wrap},
};
//AI_END===
use std::time::Duration;
use water_ball_tool_lib::music_play::effect::EffectChainKind;

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
    //AI=== 进度条为主，音量条放在同一行右侧作为次要信息 / the progress bar is primary; a compact volume bar sits on its right
    let [progress, gap, volume] = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(30),
    ])
    .areas(progress);
    render_progress(app, frame, progress);
    frame.render_widget(
        Paragraph::new(Span::styled("│", Style::new().fg(Color::DarkGray))),
        gap,
    );
    render_volume(app, frame, volume);
    //AI_END===

    let [playlist, reserved] =
        Layout::horizontal([Constraint::Percentage(42), Constraint::Percentage(58)]).areas(body);
    render_playlist(app, frame, playlist);
    //AI=== 右侧区域保留给后续功能，日志已移到 Debug 窗口 / the right side is reserved for later; logs moved into the debug window
    render_reserved(frame, reserved);
    //AI_END===

    //AI=== 底栏：命令输入、错误提示或快捷键 / footer: command input, error notice or key hints
    render_footer(app, frame, footer);
    //AI_END===

    //AI=== 互斥窗口：只渲染当前窗口 / mutually exclusive windows: render only the active one
    match app.window {
        Window::Debug => render_debug(app, frame),
        Window::Effects => render_effects(app, frame),
        Window::Help => render_help(frame),
        Window::None => {}
    }
    //AI_END===
}

//AI=== 底栏提示项：文字与可点击动作 / footer hints: label and clickable action
const FOOTER_HINTS: &[(FooterAction, &str)] = &[
    (FooterAction::Quit, "q 退出"),
    (FooterAction::Next, "n 下一曲"),
    (FooterAction::Toggle, "空格 播放/暂停"),
    (FooterAction::Debug, "F12 调试"),
    //AI=== 底栏效果器按钮 / footer effects button
    (FooterAction::Effects, "e 效果"),
    //AI_END===
    (FooterAction::Command, ": 命令"),
];
const FOOTER_GAP: &str = "  ";
//AI_END===

//AI=== 底栏渲染：`:<输入>` 左对齐，其余居中；快捷键文字本身可点击
// Footer: `:<input>` is left-aligned, everything else centered; the hint labels are clickable.
fn render_footer(app: &mut App, frame: &mut Frame, area: Rect) {
    app.footer.area = area;
    app.footer.actions.clear();
    let (line, alignment) = if let Some(input) = &app.command.input {
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
    } else if let Some(message) = &app.command.error {
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
            app.footer
                .actions
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
    let (state, state_color) = if app.playback.poisoned {
        ("异常暂停中", Color::Red)
    } else if app.playback.playing {
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
            Line::from(format!(" {}  ·  {} ", app.playback.mode, state))
                .right_aligned()
                .style(Style::new().fg(state_color)),
        );

    let mut lines = vec![
        Line::from(Span::styled(
            if app.playback.title.is_empty() {
                "等待曲目… / waiting for a track".to_string()
            } else {
                app.playback.title.clone()
            },
            Style::new().fg(Color::White).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            app.playback.artist.clone(),
            Style::new().fg(Color::LightBlue),
        )),
        Line::from(Span::styled(
            app.playback.album.clone(),
            Style::new().fg(Color::Gray),
        )),
        Line::from(Span::styled(
            app.playback.audio_info.clone(),
            Style::new().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            app.playback.path.clone(),
            Style::new().fg(Color::DarkGray),
        )),
    ];

    if app.playback.poisoned {
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
    let preview = app.seek.gesture.is_some();
    let ratio = if app.playback.duration.is_zero() {
        0.0
    } else {
        (pos.as_secs_f64() / app.playback.duration.as_secs_f64()).clamp(0.0, 1.0)
    };

    //AI=== 与 wbfp 共用的八分块进度条 / shared eighth-block bar with wbfp
    let color = if preview { Color::Yellow } else { Color::Cyan };
    let label = format!(
        " {} / {}  ·  {:.0}%{}  ·  ←/→ 跳转 ",
        fmt_time(pos),
        fmt_time(app.playback.duration),
        ratio * 100.0,
        if preview { "  ·  跳转预览" } else { "" }
    );
    crate::command::render_eighth_block_bar(
        frame,
        area,
        ratio,
        color,
        &label,
        Style::new().fg(Color::White),
    );
    //AI_END===
    //AI_END===
}

//AI=== 紧凑音量条：右侧次要信息，避免与主进度条竞争视觉 / compact volume bar: secondary right-side info, not competing with the main progress bar
fn render_volume(app: &App, frame: &mut Frame, area: Rect) {
    frame.render_widget(
        Gauge::default()
            .gauge_style(Style::new().fg(Color::Gray))
            .ratio(f64::from(app.playback.volume).clamp(0.0, 1.0))
            .label(Span::styled(
                format!(" 音量 {:.0}%  +/- ", app.playback.volume * 100.0),
                Style::new().fg(Color::White),
            )),
        area,
    );
}
//AI_END===

//AI=== 播放列表视口：只渲染可见行，记录区域供鼠标命中 / playlist viewport: render visible rows only, record the area for mouse hit-testing
fn render_playlist(app: &mut App, frame: &mut Frame, area: Rect) {
    let visible = usize::from(area.height.saturating_sub(2));
    app.playlist.area = area;
    app.playlist.height = visible;
    // 列表缩短或窗口变高后把偏移夹回有效范围 / clamp the offset after a shrink or a taller terminal
    app.scroll_playlist_to(app.playlist.offset);

    let items: Vec<ListItem> = app
        .playlist
        .tracks
        .iter()
        .enumerate()
        .skip(app.playlist.offset)
        .take(visible)
        .map(|(i, track)| {
            let selected = app.playlist.current == Some(i);
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

    let mut title = if app.scan.done {
        format!(" 播放列表 ({})", app.playlist.tracks.len())
    } else {
        format!(
            " 播放列表 ({}) · 发现 {}/{}",
            app.playlist.tracks.len(),
            app.scan.discovery.0,
            app.scan.discovery.1
        )
    };
    if app.playlist.tracks.len() > visible && visible > 0 {
        title.push_str(&format!(
            " · {}-{}/{}",
            app.playlist.offset + 1,
            (app.playlist.offset + visible).min(app.playlist.tracks.len()),
            app.playlist.tracks.len()
        ));
    }
    title.push(' ');
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(Color::Cyan))
        .title(Line::from(title).style(Style::new().fg(Color::Cyan)))
        //AI=== 上下键提示放在播放列表标题 / up/down hint lives on the playlist title
        .title(
            Line::from(" ↑/↓ 滚动 ")
                .right_aligned()
                .style(Style::new().fg(Color::DarkGray)),
        );
    //AI_END===

    frame.render_widget(List::new(items).block(block), area);
}
//AI_END===

//AI=== 主页面右侧预留区域；日志已移入 Debug 窗口 / reserved main-page right side; logs moved into the debug window
fn render_reserved(frame: &mut Frame, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(Color::Cyan))
        .title(Line::from(" 预留 ").style(Style::new().fg(Color::Cyan)));
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "右侧区域保留，后续使用",
            Style::new().fg(Color::DarkGray),
        )))
        .block(block)
        .alignment(Alignment::Center),
        area,
    );
}
//AI_END===

//AI=== 日志行：Debug 窗口右栏 / log lines for the debug window's right pane
fn log_lines(app: &App, height: u16) -> Vec<Line<'_>> {
    // 只渲染能放下的最后若干行，避免 1000 行缓存从最早的行开始显示
    // Render only the last lines that fit, so the 1000-line buffer does not start from the oldest entry.
    let visible = usize::from(height.saturating_sub(1));
    let skip = app.logs.len().saturating_sub(visible);
    app.logs.iter().skip(skip).map(log_line).collect()
}
//AI_END===

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

    //AI=== 管道宽度按 Debug 左栏计算，避免被右栏日志挤掉 / size the pipeline from the debug left column, not the whole popup
    let inner_width = popup.width.saturating_sub(2);
    let left_width = inner_width.saturating_mul(56) / 100;
    //AI_END===

    let debug = &app.debug;
    let format = debug.decoder_format.map_or_else(
        || "—".to_string(),
        |(rate, channels)| format!("{rate} Hz / {channels} ch"),
    );
    //AI=== 播放格式 / playback format
    let playback_format = debug.playback_format.map_or_else(
        || "—".to_string(),
        |(rate, channels)| format!("{rate} Hz / {channels} ch"),
    );
    //AI_END===
    let state = if app.playback.poisoned {
        "异常暂停中 / poisoned"
    } else if app.playback.playing {
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
        //AI=== 播放格式 / playback format
        debug_kv("播放格式", playback_format),
        //AI_END===
        //AI=== 两个缓冲区统一按“已用 / 容量”展示 / both buffers show occupancy over capacity
        debug_kv(
            "缓冲区",
            format!(
                "pre {} / {}  ·  audio {} / {} samples",
                debug.pre_effect_len,
                debug.pre_effect_capacity,
                debug.audio_len,
                debug.audio_capacity
            ),
        ),
        //AI_END===
        debug_kv(
            "播放列表",
            format!("{} 首 · 当前 {}", debug.playlist_len, current),
        ),
        debug_kv(
            "进度",
            format!(
                "{} / {}",
                fmt_time(app.playback.pos),
                fmt_time(app.playback.duration)
            ),
        ),
        debug_kv("音量", format!("{:.0}%", app.playback.volume * 100.0)),
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
                app.scan.discovery.0,
                app.scan.discovery.1,
                app.logs.len()
            ),
        ),
    ];

    //AI=== 延迟管道：按比例宽度绘制，左端解码、右端播放 / latency pipeline: proportional widths, decode on the left, playback on the right
    lines.extend(latency_lines(
        debug.decoder_buffer_latency,
        debug.pre_effect_queue_latency,
        debug.effect_latency,
        debug.playback_latency,
        debug.total_latency,
        usize::from(left_width.saturating_sub(20)).clamp(12, 60),
    ));
    //AI_END===

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "各段耗时（窗口最大 / 平均，ms）",
        Style::new().fg(Color::Green),
    )));

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
            Line::from(" Debug · F12 关闭 ")
                .style(Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        );
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    //AI=== Debug 左右布局：左侧状态与耗时，右侧日志 / debug is two-column: state/timings on the left, logs on the right
    let [left, gap, right] = Layout::horizontal([
        Constraint::Percentage(56),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(inner);
    frame.render_widget(Paragraph::new(lines), left);
    frame.render_widget(
        Block::default()
            .borders(Borders::LEFT)
            .border_style(Style::new().fg(Color::DarkGray)),
        gap,
    );
    let mut log_view = vec![Line::from(Span::styled(
        "日志",
        Style::new().fg(Color::Green).add_modifier(Modifier::BOLD),
    ))];
    log_view.extend(log_lines(app, right.height.saturating_sub(1)));
    frame.render_widget(Paragraph::new(log_view), right);
    //AI_END===
}

fn debug_kv(label: &'static str, value: String) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label:<10}"), Style::new().fg(Color::Green)),
        Span::styled(value, Style::new().fg(Color::Gray)),
    ])
}
//AI_END===

//AI=== 效果器窗口：左侧列表 + 右侧详情，设置区预留在详情内 / effects window: left list plus right detail, settings reserved in the detail pane
fn render_effects(app: &App, frame: &mut Frame) {
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

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(Color::Cyan))
        .title(
            Line::from(" 效果器 · ↑/↓ 选择 · 空格 开关 · e/Esc 关闭 ")
                .style(Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        );
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let [list_area, gap, detail_area] = Layout::horizontal([
        Constraint::Percentage(38),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(inner);

    let mut items = Vec::new();
    let mut selected_item = None;
    let mut chain = None;
    for (index, effect) in app.effects.list.iter().enumerate() {
        if chain != Some(effect.chain) {
            chain = Some(effect.chain);
            items.push(ListItem::new(Line::from(Span::styled(
                chain_title(effect.chain),
                Style::new().fg(Color::Green).add_modifier(Modifier::BOLD),
            ))));
        }
        if index == app.effects.selected {
            selected_item = Some(items.len());
        }
        let style = if effect.enabled {
            Style::new().fg(Color::Gray)
        } else {
            Style::new().fg(Color::DarkGray)
        };
        let state = if !effect.can_disable {
            "固定"
        } else if effect.enabled {
            "启用"
        } else {
            "禁用"
        };
        items.push(
            ListItem::new(Line::from(vec![
                Span::raw(effect.name),
                Span::raw("  "),
                Span::styled(format!("[{state}]"), style),
            ]))
            .style(style),
        );
    }
    if items.is_empty() {
        items.push(ListItem::new(Line::from(Span::styled(
            "（无效果器）",
            Style::new().fg(Color::DarkGray),
        ))));
    }

    let mut state = ListState::default();
    state.select(selected_item);
    let list = List::new(items)
        .highlight_style(
            Style::new()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");
    frame.render_stateful_widget(list, list_area, &mut state);

    frame.render_widget(
        Block::default()
            .borders(Borders::LEFT)
            .border_style(Style::new().fg(Color::DarkGray)),
        gap,
    );

    frame.render_widget(
        Paragraph::new(effect_detail_lines(app)).wrap(Wrap { trim: true }),
        detail_area,
    );
}

fn effect_detail_lines(app: &App) -> Vec<Line<'static>> {
    let Some(effect) = app.effects.list.get(app.effects.selected) else {
        return vec![Line::from(Span::styled(
            "（无效果器）",
            Style::new().fg(Color::DarkGray),
        ))];
    };
    let state = if effect.enabled { "启用" } else { "禁用" };
    let state_color = if effect.enabled {
        Color::Green
    } else {
        Color::DarkGray
    };
    let latency = if effect.latency.is_zero() {
        "—".to_string()
    } else {
        fmt_ms(effect.latency)
    };
    vec![
        Line::from(vec![
            Span::styled(
                effect.name,
                Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(format!("[{state}]"), Style::new().fg(state_color)),
        ]),
        Line::from(Span::styled(
            format!("{} · 延迟 {latency}", chain_title(effect.chain)),
            Style::new().fg(Color::Gray),
        )),
        Line::from(""),
        Line::from(Span::styled(
            effect.description,
            Style::new().fg(Color::Gray),
        )),
        Line::from(""),
        Line::from(Span::styled("── 设置 ──", Style::new().fg(Color::Green))),
        Line::from(Span::styled(
            if effect.can_disable {
                "空格  启用 / 禁用"
            } else {
                "不可禁用（需保持输出格式）"
            },
            Style::new().fg(Color::Gray),
        )),
        Line::from(Span::styled(
            "其他设置后续支持",
            Style::new().fg(Color::DarkGray),
        )),
    ]
}

fn chain_title(chain: EffectChainKind) -> &'static str {
    match chain {
        EffectChainKind::Pre => "前效果器",
        EffectChainKind::Post => "后效果器",
    }
}
//AI_END===

//AI=== 延迟以毫秒显示 / latency is shown in milliseconds
fn fmt_ms(d: Duration) -> String {
    format!("{:.1} ms", d.as_secs_f64() * 1000.0)
}
//AI_END===

//AI=== 延迟管道：分段宽度按时长比例，颜色对应各段 / latency pipeline: segment widths proportional to duration
fn latency_lines(
    decoder: Duration,
    queue: Duration,
    effect: Duration,
    playback: Duration,
    total: Duration,
    width: usize,
) -> Vec<Line<'static>> {
    let segments = [
        ("解码", decoder, Color::Cyan),
        ("排队", queue, Color::Yellow),
        ("前效果", effect, Color::Magenta),
        ("播放", playback, Color::Blue),
    ];
    let total_secs = total.as_secs_f64();
    let mut widths = [0usize; 4];
    if total_secs > 0.0 {
        for (slot, (_, value, _)) in widths.iter_mut().zip(segments) {
            *slot = ((value.as_secs_f64() / total_secs) * width as f64).round() as usize;
            if !value.is_zero() {
                *slot = (*slot).max(1);
            }
        }
        let used: usize = widths.iter().sum();
        if used > width {
            widths[3] = widths[3].saturating_sub(used - width);
        } else if let Some((index, _)) = widths.iter().enumerate().max_by_key(|(_, w)| **w) {
            widths[index] += width - used;
        }
    }

    let mut bar = vec![Span::styled("延迟管道  ", Style::new().fg(Color::Green))];
    if total.is_zero() {
        bar.push(Span::styled("—", Style::new().fg(Color::DarkGray)));
    } else {
        for ((_, _, color), segment_width) in segments.iter().zip(widths) {
            if segment_width > 0 {
                bar.push(Span::styled(
                    "█".repeat(segment_width),
                    Style::new().fg(*color),
                ));
            }
        }
    }
    bar.push(Span::styled(
        format!(" {}", fmt_ms(total)),
        Style::new().fg(Color::Gray),
    ));

    let mut legend = vec![Span::raw("          ")];
    for (index, (label, value, color)) in segments.iter().enumerate() {
        if index > 0 {
            legend.push(Span::styled(" → ", Style::new().fg(Color::DarkGray)));
        }
        legend.push(Span::styled(*label, Style::new().fg(*color)));
        legend.push(Span::styled(
            format!(" {}", fmt_ms(*value)),
            Style::new().fg(Color::Gray),
        ));
    }

    vec![Line::from(bar), Line::from(legend)]
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
