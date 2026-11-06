//AI=== seek 手势测试的导入 / imports for the seek gesture tests
use super::app::{App, FooterAction, SeekGesture};
//AI_END===
use super::channel_writer::{LogLevel, LogLine};
use super::ui;
use super::{
    SHUFFLE_INIT_TRACKS, VtEvent, VtInput, parse_time, parse_volume, resolve_command, seek_by,
    seek_target, should_start,
};
use crate::command::music_play::runtime::{DebugInfo, Snapshot};
use lofty::file::FileType;
//AI=== 鼠标报文重组与视口测试所需导入 / imports for mouse reassembly and viewport tests
use ratatui::crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::layout::Rect;
//AI_END===
use ratatui::{Terminal, backend::TestBackend, buffer::Buffer, style::Color};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use water_ball_tool_lib::music_play::LoopMode;
use water_ball_tool_lib::music_play::play_list::Track;
use water_ball_tool_lib::music_play::tools::DurationStatsWindow;

#[test]
fn preview_renders_all_panels() -> Result<(), Box<dyn std::error::Error>> {
    let mut terminal = Terminal::new(TestBackend::new(100, 30))?;

    let mut app = App::preview();
    terminal.draw(|frame| ui::render(&mut app, frame))?;

    // 宽字符在 Buffer 中占两格并在后一格补空格，断言前先去掉空格
    // Wide chars occupy two cells with a padding space in the buffer; strip spaces before asserting.
    let compact = buffer_text(terminal.backend().buffer()).replace(' ', "");
    assert!(compact.contains("正在播放"));
    assert!(compact.contains("示例曲目标题"));
    assert!(compact.contains("示例艺术家"));
    assert!(compact.contains("示例专辑"));
    assert!(compact.contains("FLAC44.1kHz/2ch"));
    assert!(compact.contains("播放列表(8)"));
    assert!(compact.contains("日志"));
    assert!(compact.contains("01:23/04:56"));
    assert!(compact.contains("q退出"));
    Ok(())
}

#[test]
fn warn_log_level_is_yellow() -> Result<(), Box<dyn std::error::Error>> {
    let mut terminal = Terminal::new(TestBackend::new(100, 30))?;
    let mut app = App::preview();
    app.push_log(LogLine {
        level: Some(LogLevel::Warn),
        text: "2026-01-01T00:00:00Z  WARN 占位警告".to_string(),
    });
    terminal.draw(|frame| ui::render(&mut app, frame))?;

    let buffer = terminal.backend().buffer();
    let mut found = false;
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width.saturating_sub(3) {
            if buffer[(x, y)].symbol() == "W"
                && buffer[(x + 1, y)].symbol() == "A"
                && buffer[(x + 2, y)].symbol() == "R"
                && buffer[(x + 3, y)].symbol() == "N"
            {
                assert_eq!(buffer[(x, y)].fg, Color::Yellow);
                found = true;
            }
        }
    }
    assert!(found);
    Ok(())
}

#[test]
fn apply_maps_snapshot_and_clears_without_track() {
    let mut app = App::preview();

    let mut track = Track::new(
        PathBuf::from("sample.flac"),
        "Title".to_string(),
        "Artist".to_string(),
        Duration::from_secs(90),
        FileType::Flac,
    );
    track.set_album(Some("Album".to_string()));
    track.set_audio_info("FLAC · 44.1 kHz · 2 ch".to_string());

    app.apply(Snapshot {
        track: Some(track),
        pos: Duration::from_secs(10),
        volume: 0.5,
        playing: true,
        poisoned: false,
        mode: LoopMode::Shuffle,
        index: Some(2),
        debug: DebugInfo::default(),
    });
    assert_eq!("Title", app.title);
    assert_eq!("Artist", app.artist);
    assert_eq!("Album", app.album);
    assert_eq!("FLAC · 44.1 kHz · 2 ch", app.audio_info);
    assert_eq!("sample.flac", app.path);
    assert_eq!(Duration::from_secs(90), app.duration);
    assert_eq!(Duration::from_secs(10), app.pos);
    assert_eq!(0.5, app.volume);
    assert!(app.playing);
    assert!(!app.poisoned);
    assert_eq!(Some(2), app.current);
    assert_eq!("随机播放", app.mode);

    app.apply(Snapshot {
        track: None,
        pos: Duration::ZERO,
        volume: 0.5,
        playing: false,
        poisoned: true,
        mode: LoopMode::SequentialLoop,
        index: None,
        debug: DebugInfo::default(),
    });
    assert_eq!(app.title, "");
    assert_eq!(app.artist, "");
    assert_eq!(app.album, "");
    assert_eq!(app.audio_info, "");
    assert_eq!(app.path, "");
    assert_eq!(Duration::ZERO, app.duration);
    assert!(app.poisoned);
    assert_eq!(None, app.current);
}

#[test]
fn poisoned_state_renders_error_line() -> Result<(), Box<dyn std::error::Error>> {
    let mut terminal = Terminal::new(TestBackend::new(100, 30))?;
    let mut app = App::preview();
    app.poisoned = true;
    terminal.draw(|frame| ui::render(&mut app, frame))?;

    let compact = buffer_text(terminal.backend().buffer()).replace(' ', "");
    assert!(compact.contains("Error"));
    assert!(compact.contains("解码错误，按空格换曲恢复"));
    assert!(compact.contains("异常暂停中"));
    Ok(())
}

#[test]
fn shuffle_startup_waits_for_five_tracks_with_done_fallback() {
    assert!(!should_start(false, true, 0, false));
    for loaded in 1..SHUFFLE_INIT_TRACKS {
        assert!(!should_start(false, true, loaded, false));
    }
    assert!(should_start(false, true, SHUFFLE_INIT_TRACKS, false));
    // 扫描结束时不足 5 首也要初始化 / initialize with fewer than five tracks once the scan ends
    assert!(should_start(false, true, 3, true));

    // 非随机模式第一首即可初始化 / non-shuffle starts with the first track
    assert!(should_start(false, false, 1, false));
    // 已初始化或没有曲目 / already started or nothing loaded
    assert!(!should_start(true, false, 1, false));
    assert!(!should_start(true, true, SHUFFLE_INIT_TRACKS, true));
    assert!(!should_start(false, false, 0, true));
}

#[test]
fn debug_popup_renders_buffers_and_timings() -> Result<(), Box<dyn std::error::Error>> {
    let mut terminal = Terminal::new(TestBackend::new(100, 30))?;
    let mut app = App::preview();
    app.show_debug = true;
    app.debug.decoder_path = Some("D:\\Music\\sample.flac".to_string());
    app.debug.decoder_format = Some((44_100, 2));
    app.debug.pre_effect_len = 123;
    app.debug.pre_effect_capacity = 4096;
    app.debug.audio_len = 4800;
    app.debug.playlist_len = 8;
    app.debug.playlist_index = Some(0);
    app.debug.metrics = vec![(
        "decode",
        DurationStatsWindow {
            total_ns: 2_000_000,
            count: 2,
            max_ns: 1_500_000,
        },
    )];

    terminal.draw(|frame| ui::render(&mut app, frame))?;

    let compact = buffer_text(terminal.backend().buffer()).replace(' ', "");
    assert!(compact.contains("Debug"));
    assert!(compact.contains("解码歌曲"));
    assert!(compact.contains("pre123/4096"));
    assert!(compact.contains("audio4800"));
    assert!(compact.contains("decode"));
    assert!(compact.contains("max1.500"));
    Ok(())
}

//AI=== 播放列表滚动 / playlist scrolling
#[test]
fn playlist_scroll_clamps_to_bounds() {
    let mut app = App::preview();
    app.playlist_height = 3;

    app.scroll_playlist(-1);
    assert_eq!(0, app.playlist_offset);

    app.scroll_playlist(100);
    assert_eq!(5, app.playlist_offset); // 8 首 - 3 行可见 / 8 tracks - 3 visible rows

    app.scroll_playlist_to(usize::MAX);
    assert_eq!(5, app.playlist_offset);

    app.scroll_playlist_to(0);
    assert_eq!(0, app.playlist_offset);
}

#[test]
fn playlist_page_scroll_moves_one_viewport() {
    let mut app = App::preview();
    app.playlist_height = 3;

    app.scroll_playlist_page(1);
    assert_eq!(3, app.playlist_offset);
    app.scroll_playlist_page(1);
    assert_eq!(5, app.playlist_offset);
    app.scroll_playlist_page(-1);
    assert_eq!(2, app.playlist_offset);
}

#[test]
fn apply_follows_current_track() {
    let mut app = App::preview();
    app.playlist_height = 3;
    app.current = Some(0);

    app.apply(Snapshot {
        track: None,
        pos: Duration::ZERO,
        volume: 0.5,
        playing: false,
        poisoned: false,
        mode: LoopMode::SequentialLoop,
        index: Some(7),
        debug: DebugInfo::default(),
    });

    assert_eq!(Some(7), app.current);
    assert_eq!(5, app.playlist_offset); // 第 8 首进入视口底部 / the 8th track enters at the bottom
}

#[test]
fn playlist_hit_test_maps_rows_and_rejects_border() {
    let mut app = App::preview();
    app.playlist_area = Rect::new(10, 5, 20, 5); // 3 行内容 + 上下边框 / 3 content rows plus borders
    app.playlist_offset = 2;

    assert_eq!(Some(2), app.playlist_index_at(10, 6));
    assert_eq!(Some(4), app.playlist_index_at(29, 8));
    // 边框、区域外、末行之后 / border, outside the area, past the last row
    assert_eq!(None, app.playlist_index_at(10, 5));
    assert_eq!(None, app.playlist_index_at(10, 9));
    assert_eq!(None, app.playlist_index_at(9, 6));
    assert_eq!(None, app.playlist_index_at(30, 6));

    // 偏移超出列表长度时不着色任何行 / no row when the offset is past the list
    app.playlist_offset = 7;
    assert_eq!(None, app.playlist_index_at(10, 7));
}

#[test]
fn playlist_render_shows_only_viewport() -> Result<(), Box<dyn std::error::Error>> {
    let mut terminal = Terminal::new(TestBackend::new(100, 14))?;
    let mut app = App::preview();
    app.playlist_offset = 6;
    terminal.draw(|frame| ui::render(&mut app, frame))?;

    let compact = buffer_text(terminal.backend().buffer()).replace(' ', "");
    assert!(compact.contains("示例曲目07"));
    assert!(compact.contains("示例曲目08"));
    assert!(!compact.contains("示例曲目01"));
    assert!(compact.contains("7-8/8"));
    Ok(())
}
//AI_END===

//AI=== Windows VT 输入解析 / Windows VT input parsing
#[test]
fn vt_sgr_mouse_report_becomes_scroll_event() {
    let mut vt = VtInput::new();
    vt.feed(b"\x1b[<64;1;1M");

    let events = vt.drain();
    assert_eq!(1, events.len());
    match events[0] {
        VtEvent::Mouse(MouseEventKind::ScrollUp, column, row) => {
            assert_eq!(0, column);
            assert_eq!(0, row);
        }
        _ => panic!("expected a scroll-up mouse event"),
    }
}

#[test]
fn vt_left_click_maps_coordinates() {
    let mut vt = VtInput::new();
    vt.feed(b"\x1b[<0;11;23M");

    let events = vt.drain();
    assert_eq!(1, events.len());
    match events[0] {
        VtEvent::Mouse(MouseEventKind::Down(MouseButton::Left), column, row) => {
            assert_eq!(10, column); // 11 - 1
            assert_eq!(22, row); // 23 - 1
        }
        _ => panic!("expected a left-button press"),
    }
}

#[test]
fn vt_cr_and_lf_are_enter() {
    let mut vt = VtInput::new();
    vt.feed(b"\r");
    assert!(matches!(
        vt.drain().first(),
        Some(VtEvent::Key(KeyCode::Enter, _))
    ));
    vt.feed(b"\n");
    assert!(matches!(
        vt.drain().first(),
        Some(VtEvent::Key(KeyCode::Enter, _))
    ));
}

#[test]
fn vt_split_sequence_is_reassembled() {
    // 终端可能把 ESC 和后续字节分两次读取 / the terminal may deliver ESC and the rest in separate reads
    let mut vt = VtInput::new();
    vt.feed(b"\x1b");
    assert!(vt.drain().is_empty());

    vt.feed(b"[C");
    assert!(matches!(
        vt.drain().first(),
        Some(VtEvent::Key(KeyCode::Right, _))
    ));
}

#[test]
fn vt_lone_escape_waits_for_timeout() {
    let mut vt = VtInput::new();
    vt.feed(b"\x1b");
    assert!(vt.drain().is_empty());
    assert!(vt.take_lone_esc(Duration::ZERO));
    assert!(matches!(
        vt.drain().first(),
        Some(VtEvent::Key(KeyCode::Esc, _))
    ));

    // ESC 后紧跟普通按键：Esc 与按键都要保留 / ESC followed by a normal key keeps both
    vt.feed(b"\x1b");
    vt.feed(b"q");
    let events = vt.drain();
    assert!(matches!(
        events.first(),
        Some(VtEvent::Key(KeyCode::Esc, _))
    ));
    assert!(matches!(
        events.get(1),
        Some(VtEvent::Key(KeyCode::Char('q'), _))
    ));
}

#[test]
fn vt_payload_q_is_not_handled_as_quit() {
    let mut vt = VtInput::new();
    // 坐标 113/33 中不含 q；用 113 模拟以前的坐标字节 q（0x71）
    // Use 113 as the coordinate that used to arrive as the q byte (0x71).
    vt.feed(b"\x1b[<35;113;33M");
    assert!(vt.drain().is_empty()); // 移动报文被忽略 / motion report ignored

    vt.feed(b"q");
    let events = vt.drain();
    assert_eq!(1, events.len());
    assert!(matches!(
        events[0],
        VtEvent::Key(KeyCode::Char('q'), KeyModifiers::NONE)
    ));
}

#[test]
fn vt_arrow_page_and_function_keys() {
    let mut vt = VtInput::new();
    vt.feed(b"\x1b[A");
    assert!(matches!(
        vt.drain().first(),
        Some(VtEvent::Key(KeyCode::Up, _))
    ));

    vt.feed(b"\x1b[5~");
    assert!(matches!(
        vt.drain().first(),
        Some(VtEvent::Key(KeyCode::PageUp, _))
    ));

    vt.feed(b"\x1b[24~");
    assert!(matches!(
        vt.drain().first(),
        Some(VtEvent::Key(KeyCode::F(12), _))
    ));
}

#[test]
fn vt_x10_mouse_fallback_is_consumed() {
    let mut vt = VtInput::new();
    // 不支持 SGR 的终端：CSI M + 按钮/坐标字节，坐标里的 q 不能被当成按键
    // Terminals without SGR: CSI M plus button/coordinate bytes; a q byte must not become a key.
    vt.feed(b"\x1b[MCq!");
    assert!(vt.drain().is_empty());

    vt.feed(b"q");
    assert!(matches!(
        vt.drain().first(),
        Some(VtEvent::Key(KeyCode::Char('q'), _))
    ));
}

#[test]
fn vt_ctrl_c_is_preserved() {
    let mut vt = VtInput::new();
    vt.feed(&[0x03]);

    let events = vt.drain();
    assert_eq!(1, events.len());
    assert!(matches!(
        events[0],
        VtEvent::Key(KeyCode::Char('c'), KeyModifiers::CONTROL)
    ));
}
//AI_END===

//AI=== 命令模式：前缀匹配、参数解析与底栏/帮助渲染 / command mode: prefix match, argument parsing, footer and help rendering
#[test]
fn command_prefix_must_be_unique() {
    assert_eq!(Ok("play"), resolve_command("pl"));
    assert_eq!(Ok("pause"), resolve_command("pa"));
    assert_eq!(Ok("prev"), resolve_command("pr"));
    assert_eq!(Ok("next"), resolve_command("n"));
    assert_eq!(Ok("quit"), resolve_command("q"));
    assert_eq!(Ok("help"), resolve_command("?"));
    assert_eq!(Ok("vol"), resolve_command("v"));
    assert_eq!(Ok("seek"), resolve_command("s"));

    // 歧义与未知命令都要报错 / ambiguity and unknown names are errors
    let Err(message) = resolve_command("p") else {
        panic!("expected an ambiguous command");
    };
    assert!(message.contains("命令不唯一"));
    let Err(message) = resolve_command("zzz") else {
        panic!("expected an unknown command");
    };
    assert!(message.contains("未知命令"));
}

#[test]
fn volume_argument_accepts_absolute_and_relative_values() {
    assert_eq!(Ok(0.5), parse_volume("50", 0.25));

    let Ok(up) = parse_volume("+5", 0.5) else {
        panic!("relative increase must parse");
    };
    assert!((up - 0.55).abs() < 1e-6);
    let Ok(down) = parse_volume("-5", 0.5) else {
        panic!("relative decrease must parse");
    };
    assert!((down - 0.45).abs() < 1e-6);

    // 越界夹取与非法输入 / clamped bounds and invalid input
    assert_eq!(Ok(0.0), parse_volume("-200", 0.5));
    assert_eq!(Ok(1.0), parse_volume("150", 0.5));
    assert!(parse_volume("abc", 0.5).is_err());
}

#[test]
fn seek_time_argument_accepts_seconds_and_clock() {
    assert_eq!(Ok(Duration::from_secs(83)), parse_time("83"));
    assert_eq!(Ok(Duration::from_secs(83)), parse_time("1:23"));
    assert_eq!(Ok(Duration::from_secs(3_723)), parse_time("1:02:03"));

    // 空串、非数字与负号（相对号由命令层剥离）都要报错
    // Empty input, non-numeric parts and a sign (the command layer strips it) are errors.
    assert!(parse_time("").is_err());
    assert!(parse_time("1:xx").is_err());
    assert!(parse_time("-5").is_err());
}

#[test]
fn seek_target_steps_and_clamps() {
    assert_eq!(
        Duration::ZERO,
        seek_target(Duration::from_secs(1), -1, Duration::from_secs(100))
    );
    assert_eq!(
        Duration::from_secs(100),
        seek_target(Duration::from_secs(99), 1, Duration::from_secs(100))
    );
    // 时长未知时只做步进，不设上限 / unknown duration only steps, without an upper bound
    assert_eq!(
        Duration::from_secs(8),
        seek_target(Duration::from_secs(3), 1, Duration::ZERO)
    );

    // 长按偏移与点按使用同一套夹取 / the hold offset shares the tap clamping
    assert_eq!(
        Duration::from_secs(25),
        seek_by(
            Duration::from_secs(10),
            1,
            Duration::from_secs(15),
            Duration::from_secs(100)
        )
    );
    assert_eq!(
        Duration::ZERO,
        seek_by(
            Duration::from_secs(10),
            -1,
            Duration::from_secs(15),
            Duration::from_secs(100)
        )
    );
}

#[test]
fn seek_preview_overrides_displayed_position() {
    let mut app = App::preview();
    app.pos = Duration::from_secs(10);
    assert_eq!(Duration::from_secs(10), app.display_pos());

    app.seek_gesture = Some(SeekGesture {
        target: Duration::from_secs(42),
        pending: true,
        last: Instant::now(),
        direction: 1,
        started: Instant::now(),
        start: Duration::from_secs(42),
    });
    assert_eq!(Duration::from_secs(42), app.display_pos());
}

#[test]
fn seek_preview_is_rendered() -> Result<(), Box<dyn std::error::Error>> {
    let mut terminal = Terminal::new(TestBackend::new(100, 30))?;
    let mut app = App::preview();
    app.seek_gesture = Some(SeekGesture {
        target: Duration::from_secs(200),
        pending: true,
        last: Instant::now(),
        direction: 1,
        started: Instant::now(),
        start: Duration::from_secs(200),
    });
    terminal.draw(|frame| ui::render(&mut app, frame))?;

    let compact = buffer_text(terminal.backend().buffer()).replace(' ', "");
    assert!(compact.contains("03:20/04:56"));
    assert!(compact.contains("跳转预览"));
    Ok(())
}

#[test]
fn footer_hints_have_click_targets() -> Result<(), Box<dyn std::error::Error>> {
    let mut terminal = Terminal::new(TestBackend::new(100, 30))?;
    let mut app = App::preview();
    terminal.draw(|frame| ui::render(&mut app, frame))?;

    assert!(app.footer_actions.len() >= 2);
    let (quit_start, _, quit_action) = app.footer_actions[0];
    let (next_start, _, next_action) = app.footer_actions[1];
    assert_eq!(FooterAction::Quit, quit_action);
    assert_eq!(FooterAction::Next, next_action);
    assert_eq!(Some(FooterAction::Quit), app.footer_action_at(quit_start));
    assert_eq!(Some(FooterAction::Next), app.footer_action_at(next_start));

    // 命令输入时底栏不是按钮 / the footer is not a button bar while typing a command
    app.command = Some("seek".to_string());
    terminal.draw(|frame| ui::render(&mut app, frame))?;
    assert!(app.footer_actions.is_empty());
    Ok(())
}

#[test]
fn command_footer_shows_input_and_error() -> Result<(), Box<dyn std::error::Error>> {
    let mut terminal = Terminal::new(TestBackend::new(100, 30))?;
    let mut app = App::preview();
    app.command = Some("vol +5".to_string());
    terminal.draw(|frame| ui::render(&mut app, frame))?;
    let compact = buffer_text(terminal.backend().buffer()).replace(' ', "");
    assert!(compact.contains(":vol+5"));

    app.command = None;
    app.command_error = Some("未知命令：zzz".to_string());
    terminal.draw(|frame| ui::render(&mut app, frame))?;
    let compact = buffer_text(terminal.backend().buffer()).replace(' ', "");
    assert!(compact.contains("未知命令：zzz"));
    Ok(())
}

#[test]
fn help_popup_lists_every_command() -> Result<(), Box<dyn std::error::Error>> {
    let mut terminal = Terminal::new(TestBackend::new(100, 30))?;
    let mut app = App::preview();
    app.show_help = true;
    terminal.draw(|frame| ui::render(&mut app, frame))?;

    let compact = buffer_text(terminal.backend().buffer()).replace(' ', "");
    assert!(compact.contains("命令帮助"));
    assert!(compact.contains(":vol<0-100|+N|-N>"));
    assert!(compact.contains("Esc关闭帮助"));
    Ok(())
}
//AI_END===

fn buffer_text(buffer: &Buffer) -> String {
    let width = buffer.area.width as usize;
    buffer
        .content
        .chunks(width)
        .map(|row| {
            let mut line: String = row.iter().map(|cell| cell.symbol()).collect();
            line.push('\n');
            line
        })
        .collect()
}
