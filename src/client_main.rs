mod client;
mod shared;

use client::app::App;
use client::commands::handle_input;
use client::network::send_json;
use client::ui::draw_ui;
use shared::protocol::*;

use serde::Deserialize;
use std::fs;

use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};

use ratatui::{Terminal, backend::CrosstermBackend};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

enum AuthMode {
    Register,
    Login,
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
            println!(
                "Unexpected server response during authentication: {:?}",
                other
            );
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
