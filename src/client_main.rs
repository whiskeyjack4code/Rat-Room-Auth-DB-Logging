mod client;
mod shared;

use shared::protocol::*;

use serde::Deserialize;
use std::fs;

use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};

use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    widgets::{Block, Borders, Paragraph},
};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpStream, tcp::OwnedWriteHalf};
use tokio::sync::mpsc;

const MAX_MESSAGES: usize = 200;

enum AuthMode {
    Register,
    Login,
}

struct App {
    messages: Vec<String>,
    input: String,
    username: String,
    room: String,
    scroll: usize,
}

#[derive(Deserialize)]
struct Config {
    host: String,
    port: u16,
}

fn load_config() -> Config {
    let contents = fs::read_to_string("client.toml").expect("Failed to read client.toml");

    toml::from_str(&contents).expect("Invalid client.toml format")
}

impl App {
    fn new(username: String) -> Self {
        Self {
            messages: Vec::new(),
            input: String::new(),
            username,
            room: "lobby".to_string(),
            scroll: 0,
        }
    }

    fn push_message(&mut self, message: String) {
        self.messages.push(message);

        if self.messages.len() > MAX_MESSAGES {
            let overflow = self.messages.len() - MAX_MESSAGES;
            self.messages.drain(0..overflow);
        }

        self.scroll_to_bottom();
    }

    fn scroll_up(&mut self) {
        if self.scroll > 0 {
            self.scroll -= 1;
        }
    }

    fn scroll_down(&mut self) {
        self.scroll += 1;
    }

    fn scroll_to_bottom(&mut self) {
        self.scroll = self.messages.len();
    }
}

async fn send_json(writer: &mut OwnedWriteHalf, message: &ClientMessage) {
    if let Ok(json) = serde_json::to_string(message) {
        let _ = writer.write_all(json.as_bytes()).await;
        let _ = writer.write_all(b"\n").await;
    }
}

async fn handle_input(app: &mut App, writer: &mut OwnedWriteHalf) {
    let message = app.input.trim().to_string();

    if message.is_empty() {
        return;
    }

    if message == "/leave" {
        send_json(writer, &ClientMessage::LeaveRoom).await;
    } else if message == "/rooms" {
        send_json(writer, &ClientMessage::ListRooms).await;
    } else if let Some(room) = message.strip_prefix("/join ") {
        let room = room.trim();

        if !room.is_empty() {
            send_json(
                writer,
                &ClientMessage::JoinRoom {
                    room: room.to_string(),
                },
            )
            .await;
        }
    } else {
        send_json(
            writer,
            &ClientMessage::Chat {
                message: message.clone(),
            },
        )
        .await;
    }

    app.input.clear();
}

fn draw_ui(frame: &mut ratatui::Frame, app: &App) {
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

#[tokio::main]
async fn main() {
    let mut stdin = BufReader::new(tokio::io::stdin());

    let mut mode_input = String::new();
    println!("Type 'register' or 'login':");
    stdin.read_line(&mut mode_input).await.unwrap();

    let auth_mode = match mode_input.trim().to_lowercase().as_str() {
        "register" => AuthMode::Register,
        "login" => AuthMode::Login,
        _ => {
            println!("Invalid option. Please start again and type 'register' or 'login'.");
            return;
        }
    };

    let mut username = String::new();
    println!("Enter username:");
    stdin.read_line(&mut username).await.unwrap();
    let username = username.trim().to_string();

    let mut password = String::new();
    println!("Enter password:");
    stdin.read_line(&mut password).await.unwrap();
    let password = password.trim().to_string();

    let config = load_config();
    let address = format!("{}:{}", config.host, config.port);

    let stream = TcpStream::connect(&address)
        .await
        .expect("Failed to connect");

    println!("Connecting to {}", address);

    let (reader, mut writer) = stream.into_split();

    let auth_message = match auth_mode {
        AuthMode::Register => ClientMessage::Register {
            username: username.clone(),
            password,
        },
        AuthMode::Login => ClientMessage::Login {
            username: username.clone(),
            password,
        },
    };

    send_json(&mut writer, &auth_message).await;

    let mut reader = BufReader::new(reader);
    let mut first_response = String::new();

    match reader.read_line(&mut first_response).await {
        Ok(0) => {
            println!("Server closed the connection during authentication.");
            return;
        }
        Ok(_) => {}
        Err(e) => {
            println!("Failed to read authentication response: {e}");
            return;
        }
    }

    let auth_response: ServerMessage = match serde_json::from_str(first_response.trim()) {
        Ok(msg) => msg,
        Err(e) => {
            println!("Failed to parse authentication response: {e}");
            return;
        }
    };

    match auth_response {
        ServerMessage::AuthOk => {
            println!("Authentication successful.");
        }
        ServerMessage::AuthError { message } => {
            println!("Authentication failed: {message}");
            return;
        }
        other => {
            println!("Unexpected server response during authentication: {:?}", other);
            return;
        }
    }

    let (tx, mut rx) = mpsc::unbounded_channel();

    println!("Connected and authenticated as {}", username);
    tokio::spawn(async move {
        let mut line = String::new();

        loop {
            line.clear();

            match reader.read_line(&mut line).await {
                Ok(0) => break,
                Ok(_) => {
                    if let Ok(msg) = serde_json::from_str::<ServerMessage>(line.trim()) {
                        let _ = tx.send(msg);
                    }
                }
                Err(_) => break,
            }
        }
    });

    enable_raw_mode().unwrap();
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen).unwrap();

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).unwrap();

    let mut app = App::new(username);

    loop {
        while let Ok(msg) = rx.try_recv() {
            match msg {
                ServerMessage::AuthOk => {
                    app.push_message("[auth] Authentication successful".to_string());
                }
                ServerMessage::AuthError { message } => {
                    app.push_message(format!("[auth error] {message}"));
                }
                ServerMessage::Welcome { message } => {
                    app.push_message(format!("[welcome] {message}"));
                }
                ServerMessage::System { message } => {
                    app.push_message(format!("[system] {message}"));
                }
                ServerMessage::Chat {
                    username,
                    room,
                    message,
                } => {
                    app.push_message(format!("[{room}] {username}: {message}"));
                }
                ServerMessage::RoomJoined { room } => {
                    app.room = room.clone();
                    app.push_message(format!("[room] Joined {room}"));
                }
                ServerMessage::RoomList { rooms } => {
                    app.push_message(format!("[rooms] {}", rooms.join(", ")));
                }
                ServerMessage::Error { message } => {
                    app.push_message(format!("[error] {message}"));
                }
            }
        }

        terminal.draw(|f| draw_ui(f, &app)).unwrap();

        if event::poll(std::time::Duration::from_millis(50)).unwrap() {
            if let Event::Key(key) = event::read().unwrap() {
                if key.kind == event::KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char(c) => {
                            app.input.push(c);
                        }
                        KeyCode::Backspace => {
                            app.input.pop();
                        }
                        KeyCode::Enter => {
                            handle_input(&mut app, &mut writer).await;
                        }
                        KeyCode::Up => {
                            app.scroll_up();
                        }
                        KeyCode::Down => {
                            app.scroll_down();
                        }
                        KeyCode::Esc => break,
                        _ => {}
                    }
                }
            }
        }
    }

    disable_raw_mode().unwrap();
    execute!(terminal.backend_mut(), LeaveAlternateScreen).unwrap();
    terminal.show_cursor().unwrap();
}
