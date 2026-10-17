use crate::command::music_play::tui::app::App;
use ratatui::{
    Frame,
    style::{Color, Style},
    text::Line,
    widgets::{Block, Borders, Paragraph},
};

pub fn render(app: &App, frame: &mut Frame) {
    // 简单实现：整个屏幕就是一个日志区域
    let area = frame.area();

    let log_lines: Vec<Line> = app.logs.iter().map(|l| Line::raw(l.clone())).collect();

    let log_panel = Paragraph::new(log_lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Logs (q to quit) "),
        )
        .style(Style::default().fg(Color::Gray));

    frame.render_widget(log_panel, area);
}
