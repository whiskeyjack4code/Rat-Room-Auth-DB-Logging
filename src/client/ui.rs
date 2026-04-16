use crate::client::app::App;
use ratatui::{
    layout::{Constraint, Direction, Layout},
    widgets::{Block, Borders, Paragraph},
};

pub fn draw_ui(frame: &mut ratatui::Frame, app: &App) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),    // chat
            Constraint::Length(3), // input
            Constraint::Length(2), // status/help
        ])
        .split(frame.area());

    let chat_height = layout[0].height.saturating_sub(2) as usize;

    let total_messages = app.messages.len();
    let end = total_messages.saturating_sub(app.scroll.saturating_sub(chat_height));
    let start = end.saturating_sub(chat_height);

    let visible_messages = if start < end && end <= total_messages {
        app.messages[start..end].join("\n")
    } else {
        String::new()
    };

    let messages = Paragraph::new(visible_messages)
        .block(Block::default().borders(Borders::ALL).title("Chat"));

    frame.render_widget(messages, layout[0]);

    let input = Paragraph::new(app.input.as_str())
        .block(Block::default().borders(Borders::ALL).title("Input"));

    frame.render_widget(input, layout[1]);

    let status = Paragraph::new(format!(
        "User: {} | Room: {} | Commands: /join <room>  /leave  /rooms | Esc to quit",
        app.username, app.room
    ));

    frame.render_widget(status, layout[2]);

    let cursor_x = layout[1].x + 1 + app.input.len() as u16;
    let cursor_y = layout[1].y + 1;
    frame.set_cursor_position((cursor_x, cursor_y));
}
