mod server;
mod shared;

use shared::protocol::*;

use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Row, SqlitePool};
use std::str::FromStr;
use tracing::{error, info, warn};
use tracing_subscriber::{fmt, EnvFilter};

use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use serde::Deserialize;
use std::fs;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Mutex};

#[derive(Clone)]
struct Client {
    username: String,
    room: String,
    tx: mpsc::UnboundedSender<ServerMessage>,
}

type Clients = Arc<Mutex<HashMap<usize, Client>>>;
static NEXT_CLIENT_ID: AtomicUsize = AtomicUsize::new(1);
const DEFAULT_ROOM: &str = "lobby";

#[derive(Deserialize)]
struct Config {
    host: String,
    port: u16,
}

#[tokio::main]
async fn main() {
    init_logging();
    info!("server starting");

    let config = load_config();
    let address = format!("{}:{}", config.host, config.port);

    let listener = TcpListener::bind(&address).await.unwrap_or_else(|e| {
        error!("failed to bind to {}: {}", address, e);
        panic!("Failed to bind");
    });

    info!("listening on {}", address);

    let clients: Clients = Arc::new(Mutex::new(HashMap::new()));

    let options = SqliteConnectOptions::from_str("sqlite://chat.db")
        .expect("Invalid SQLite connection string")
        .create_if_missing(true);

    let db = SqlitePool::connect_with(options)
        .await
        .expect("Failed to connect to SQLite");

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS users (
            username TEXT PRIMARY KEY,
            password TEXT NOT NULL
        )",
    )
        .execute(&db)
        .await
        .expect("Failed to create users table");

    info!("database connected and users table ready");

    loop {
        let (socket, addr) = listener.accept().await.expect("Failed to accept");
        info!("new client connected: {}", addr);

        let clients = Arc::clone(&clients);
        let db = db.clone();

        tokio::spawn(async move {
            handle_client(socket, clients, db).await;
        });
    }
}

async fn send_json(
    writer: &mut tokio::net::tcp::OwnedWriteHalf,
    message: &ServerMessage,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let json = serde_json::to_string(message)?;

    writer.write_all(json.as_bytes()).await?;
    writer.write_all(b"\n").await?;

    Ok(())
}

fn is_valid_username(name: &str) -> bool {
    !name.trim().is_empty() && name.len() <= 20
}

async fn username_exists(clients: &Clients, username: &str) -> bool {
    let clients_guard = clients.lock().await;

    clients_guard
        .values()
        .any(|client| client.username.eq_ignore_ascii_case(username))
}

async fn broadcast_system_to_room(clients: &Clients, room: &str, message: &str) {
    let clients_guard = clients.lock().await;

    for client in clients_guard.values() {
        if client.room == room {
            let _ = client.tx.send(ServerMessage::System {
                message: message.to_string(),
            });
        }
    }
}

async fn broadcast_chat_to_room(clients: &Clients, username: &str, room: &str, message: &str) {
    let clients_guard = clients.lock().await;

    for client in clients_guard.values() {
        if client.room == room {
            let _ = client.tx.send(ServerMessage::Chat {
                username: username.to_string(),
                room: room.to_string(),
                message: message.to_string(),
            });
        }
    }
}

async fn get_client_room(clients: &Clients, client_id: usize) -> Option<String> {
    let clients_guard = clients.lock().await;
    clients_guard.get(&client_id).map(|client| client.room.clone())
}

async fn move_client_to_room(
    clients: &Clients,
    client_id: usize,
    new_room: &str,
) -> Option<(String, String)> {
    let mut clients_guard = clients.lock().await;

    let client = clients_guard.get_mut(&client_id)?;
    let old_room = client.room.clone();
    client.room = new_room.to_string();

    Some((client.username.clone(), old_room))
}

async fn list_rooms(clients: &Clients) -> Vec<String> {
    let clients_guard = clients.lock().await;

    let mut rooms: Vec<String> = clients_guard.values().map(|c| c.room.clone()).collect();

    rooms.sort();
    rooms.dedup();

    rooms
}

async fn handle_client(socket: TcpStream, clients: Clients, db: SqlitePool) {
    let client_id = NEXT_CLIENT_ID.fetch_add(1, Ordering::Relaxed);
    info!("starting session for client_id={}", client_id);

    let (reader, mut writer) = socket.into_split();
    let reader = BufReader::new(reader);
    let mut lines = reader.lines();

    let first_line = match lines.next_line().await {
        Ok(Some(line)) => line,
        Ok(None) => {
            warn!(
                "client_id={} disconnected before sending first message",
                client_id
            );
            return;
        }
        Err(e) => {
            error!("client_id={} failed reading first line: {}", client_id, e);
            return;
        }
    };

    let first_message: ClientMessage = match serde_json::from_str(&first_line) {
        Ok(msg) => msg,
        Err(e) => {
            warn!("client_id={} sent invalid initial JSON: {}", client_id, e);

            let _ = send_json(
                &mut writer,
                &ServerMessage::Error {
                    message: "Invalid protocol message".to_string(),
                },
            )
                .await;

            return;
        }
    };

    let username = match first_message {
        ClientMessage::Register { username, password } => {
            let username = username.trim().to_string();
            let password = password.trim().to_string();

            if !is_valid_username(&username) {
                warn!(
                    "client_id={} attempted invalid registration username '{}'",
                    client_id, username
                );

                let _ = send_json(
                    &mut writer,
                    &ServerMessage::AuthError {
                        message: "Invalid username".to_string(),
                    },
                )
                    .await;

                return;
            }

            if password.is_empty() {
                warn!("client_id={} attempted registration with empty password", client_id);

                let _ = send_json(
                    &mut writer,
                    &ServerMessage::AuthError {
                        message: "Password cannot be empty".to_string(),
                    },
                )
                    .await;

                return;
            }

            let salt = SaltString::generate(&mut OsRng);
            let argon2 = Argon2::default();

            let password_hash = match argon2.hash_password(password.as_bytes(), &salt) {
                Ok(hash) => hash.to_string(),
                Err(e) => {
                    error!(
                        "client_id={} failed to hash password for '{}': {}",
                        client_id, username, e
                    );

                    let _ = send_json(
                        &mut writer,
                        &ServerMessage::AuthError {
                            message: "Failed to create account".to_string(),
                        },
                    )
                        .await;

                    return;
                }
            };

            let result = sqlx::query("INSERT INTO users (username, password) VALUES (?, ?)")
                .bind(&username)
                .bind(&password_hash)
                .execute(&db)
                .await;

            match result {
                Ok(_) => {
                    info!("client_id={} registered username='{}' in DB", client_id, username);
                    username
                }
                Err(e) => {
                    warn!(
                        "client_id={} failed registration for '{}': {}",
                        client_id, username, e
                    );

                    let _ = send_json(
                        &mut writer,
                        &ServerMessage::AuthError {
                            message: "Username already exists".to_string(),
                        },
                    )
                        .await;

                    return;
                }
            }
        }

        ClientMessage::Login { username, password } => {
            let username = username.trim().to_string();
            let password = password.trim().to_string();

            if !is_valid_username(&username) {
                warn!(
                    "client_id={} attempted login with invalid username '{}'",
                    client_id, username
                );

                let _ = send_json(
                    &mut writer,
                    &ServerMessage::AuthError {
                        message: "Invalid username".to_string(),
                    },
                )
                    .await;

                return;
            }

            let row = sqlx::query("SELECT password FROM users WHERE username = ?")
                .bind(&username)
                .fetch_optional(&db)
                .await
                .expect("DB query failed");

            match row {
                Some(row) => {
                    let stored_password: String = row.get("password");

                    let parsed_hash = match PasswordHash::new(&stored_password) {
                        Ok(hash) => hash,
                        Err(e) => {
                            error!(
                                "stored password hash for '{}' is invalid: {}",
                                username, e
                            );

                            let _ = send_json(
                                &mut writer,
                                &ServerMessage::AuthError {
                                    message: "Invalid username or password".to_string(),
                                },
                            )
                                .await;

                            return;
                        }
                    };

                    if Argon2::default()
                        .verify_password(password.as_bytes(), &parsed_hash)
                        .is_err()
                    {
                        warn!("client_id={} failed login for '{}'", client_id, username);

                        let _ = send_json(
                            &mut writer,
                            &ServerMessage::AuthError {
                                message: "Invalid username or password".to_string(),
                            },
                        )
                            .await;

                        return;
                    }

                    info!("client_id={} logged in as '{}'", client_id, username);
                    username
                }
                None => {
                    warn!("client_id={} failed login for unknown user '{}'", client_id, username);

                    let _ = send_json(
                        &mut writer,
                        &ServerMessage::AuthError {
                            message: "Invalid username or password".to_string(),
                        },
                    )
                        .await;

                    return;
                }
            }
        }

        _ => {
            warn!("client_id={} first message was not register/login", client_id);

            let _ = send_json(
                &mut writer,
                &ServerMessage::AuthError {
                    message: "First message must be register or login".to_string(),
                },
            )
                .await;

            return;
        }
    };

    if username_exists(&clients, &username).await {
        warn!(
            "client_id={} tried to connect with username '{}' already active",
            client_id, username
        );

        let _ = send_json(
            &mut writer,
            &ServerMessage::AuthError {
                message: "This user is already logged in".to_string(),
            },
        )
            .await;

        return;
    }

    let _ = send_json(&mut writer, &ServerMessage::AuthOk).await;

    let (tx, mut rx) = mpsc::unbounded_channel::<ServerMessage>();

    {
        let mut clients_guard = clients.lock().await;
        clients_guard.insert(
            client_id,
            Client {
                username: username.clone(),
                room: DEFAULT_ROOM.to_string(),
                tx,
            },
        );
    }

    let _ = send_json(
        &mut writer,
        &ServerMessage::Welcome {
            message: format!("Welcome {}", username),
        },
    )
        .await;

    let _ = send_json(
        &mut writer,
        &ServerMessage::RoomJoined {
            room: DEFAULT_ROOM.to_string(),
        },
    )
        .await;

    info!(
        "client_id={} authenticated username='{}' room='{}'",
        client_id, username, DEFAULT_ROOM
    );

    let username_for_writer = username.clone();

    tokio::spawn(async move {
        while let Some(message) = rx.recv().await {
            let json = match serde_json::to_string(&message) {
                Ok(json) => json,
                Err(e) => {
                    error!(
                        "failed to serialize server message for username='{}': {}",
                        username_for_writer, e
                    );
                    continue;
                }
            };

            if let Err(e) = writer.write_all(json.as_bytes()).await {
                warn!("write failed for username='{}': {}", username_for_writer, e);
                break;
            }

            if let Err(e) = writer.write_all(b"\n").await {
                warn!(
                    "newline write failed for username='{}': {}",
                    username_for_writer, e
                );
                break;
            }
        }

        info!("writer task ended for username='{}'", username_for_writer);
    });

    broadcast_system_to_room(
        &clients,
        DEFAULT_ROOM,
        &format!("{username} joined {DEFAULT_ROOM}"),
    )
        .await;
    info!("username='{}' joined room='{}'", username, DEFAULT_ROOM);

    while let Ok(Some(line)) = lines.next_line().await {
        let parsed: ClientMessage = match serde_json::from_str(&line) {
            Ok(msg) => msg,
            Err(e) => {
                warn!("invalid JSON received from username='{}': {}", username, e);
                continue;
            }
        };

        match parsed {
            ClientMessage::Chat { message } => {
                let message = message.trim();

                if message.is_empty() {
                    continue;
                }

                if let Some(room) = get_client_room(&clients, client_id).await {
                    info!(
                        "chat room='{}' username='{}' message='{}'",
                        room, username, message
                    );
                    broadcast_chat_to_room(&clients, &username, &room, message).await;
                }
            }
            ClientMessage::Register { .. } | ClientMessage::Login { .. } => {
                warn!("username='{}' tried to authenticate mid-session", username);
            }
            ClientMessage::JoinRoom { room } => {
                let room = room.trim();

                if room.is_empty() {
                    continue;
                }

                let moved = move_client_to_room(&clients, client_id, room).await;
                if let Some((username, old_room)) = moved {
                    let _ = {
                        let clients_guard = clients.lock().await;
                        clients_guard.get(&client_id).map(|client| {
                            client.tx.send(ServerMessage::RoomJoined {
                                room: room.to_string(),
                            })
                        })
                    };

                    if old_room != room {
                        broadcast_system_to_room(
                            &clients,
                            &old_room,
                            &format!("{username} left {old_room}"),
                        )
                            .await;

                        broadcast_system_to_room(
                            &clients,
                            room,
                            &format!("{username} joined {room}"),
                        )
                            .await;
                    }

                    info!(
                        "username='{}' moved from room='{}' to room='{}'",
                        username, old_room, room
                    );
                }
            }
            ClientMessage::ListRooms => {
                info!("username='{}' requested room list", username);

                let rooms = list_rooms(&clients).await;

                let _ = {
                    let clients_guard = clients.lock().await;
                    clients_guard
                        .get(&client_id)
                        .map(|client| client.tx.send(ServerMessage::RoomList { rooms }))
                };
            }
            ClientMessage::LeaveRoom => {
                let moved = move_client_to_room(&clients, client_id, DEFAULT_ROOM).await;

                if let Some((username, old_room)) = moved {
                    let _ = {
                        let clients_guard = clients.lock().await;
                        clients_guard.get(&client_id).map(|client| {
                            client.tx.send(ServerMessage::RoomJoined {
                                room: DEFAULT_ROOM.to_string(),
                            })
                        })
                    };

                    if old_room != DEFAULT_ROOM {
                        broadcast_system_to_room(
                            &clients,
                            &old_room,
                            &format!("{username} left {old_room}"),
                        )
                            .await;

                        broadcast_system_to_room(
                            &clients,
                            DEFAULT_ROOM,
                            &format!("{username} joined {DEFAULT_ROOM}"),
                        )
                            .await;
                    }

                    info!(
                        "username='{}' moved from room='{}' to room='{}'",
                        username, old_room, DEFAULT_ROOM
                    );
                }
            }
        }
    }

    let current_room = get_client_room(&clients, client_id)
        .await
        .unwrap_or_else(|| DEFAULT_ROOM.to_string());

    {
        let mut clients_guard = clients.lock().await;
        clients_guard.remove(&client_id);
    }

    broadcast_system_to_room(
        &clients,
        &current_room,
        &format!("{username} left {current_room}"),
    )
        .await;

    info!(
        "username='{}' disconnected from room='{}'",
        username, current_room
    );
}

fn init_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    fmt().with_env_filter(filter).init();
}

fn load_config() -> Config {
    info!("loading server config from server.toml");
    let contents = fs::read_to_string("server.toml").expect("Failed to read server.toml");

    let config: Config = toml::from_str(&contents).expect("Invalid server.toml format");

    info!("server config loaded: {}:{}", config.host, config.port);

    config
}