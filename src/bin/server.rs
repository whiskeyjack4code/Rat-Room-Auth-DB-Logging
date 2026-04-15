#[path = "../protocol.rs"]
mod protocol;

use protocol::{ClientMessage, ServerMessage};

use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Mutex};
use serde_json;
use serde::Deserialize;
use std::fs;

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

fn load_config() -> Config {
    let contents = fs::read_to_string("server.toml")
        .expect("Failed to read server.toml");

    toml::from_str(&contents)
        .expect("Invalid server.toml format")
}

#[tokio::main]
async fn main() {
    let config = load_config();
    let address = format!("{}:{}", config.host, config.port);

    let listener = TcpListener::bind(&address)
        .await
        .expect("Failed to bind");

    println!("Server listening on {address}");

    let clients: Clients = Arc::new(Mutex::new(HashMap::new()));

    loop {
        let (socket, addr) = listener.accept().await.expect("Failed to accept");
        println!("New connection from {addr}");

        let clients = Arc::clone(&clients);

        tokio::spawn(async move {
            handle_client(socket, clients).await;
        });
    }
}

async fn send_json(
    writer: &mut tokio::net::tcp::OwnedWriteHalf,
    message: &ServerMessage,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let json = serde_json::to_string(&message)?;

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
        if client.room == room{
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

    let mut rooms: Vec<String> = clients_guard
        .values()
        .map(|c| c.room.clone())
        .collect();

    rooms.sort();
    rooms.dedup();

    rooms
}

async fn handle_client(socket: TcpStream, clients: Clients) {
    let client_id = NEXT_CLIENT_ID.fetch_add(1, Ordering::Relaxed);

    let (reader, mut writer) = socket.into_split();
    let reader = BufReader::new(reader);
    let mut lines = reader.lines();

    let first_line = match lines.next_line().await {
        Ok(Some(line)) => line,
        _ => return,
    };

    let first_message: ClientMessage = match serde_json::from_str(&first_line) {
        Ok(msg) => msg,
        Err(_) => {
            let _ = send_json(
                &mut writer,
                &ServerMessage::Error {
                    message: "Invalid protocol message".to_string(),
                },
            ).await;

            return;
        }
    };

    let username = match first_message {
       ClientMessage::SetUsername { username } => username.trim().to_string(),
        _ => {
            let _ = send_json(
                &mut writer,
                &ServerMessage::Error {
                    message: "First message must be 'set_username'.".to_string(),
                },
            ).await;
            return;
        }
    };

    if !is_valid_username(&username) {
        let _ = send_json(
            &mut writer,
            &ServerMessage::Error {
                message: "Invalid username".to_string(),
            },
        ).await;

        return;
    }

    if username_exists(&clients, &username).await {
        let _ = send_json(
            &mut writer,
            &ServerMessage::Error {
                message: "Username already taken".to_string(),
            },
        ).await;

        return;
    }

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
    ).await;

    let _ = send_json(
        &mut writer,
        &ServerMessage::RoomJoined {
            room: DEFAULT_ROOM.to_string(),
        },
    ).await;

    println!("Client {client_id} registered as {username}");

    tokio::spawn(async move {
        while let Some(message) = rx.recv().await {
           let json = match serde_json::to_string(&message) {
               Ok(json) => json,
               Err(_) => continue,
           };

            if writer.write_all(json.as_bytes()).await.is_err() {
                break;
            }

            if writer.write_all(b"\n").await.is_err() {
                break;
            }
        }
    });

    broadcast_system_to_room(&clients, DEFAULT_ROOM, &format!("{username} joined {DEFAULT_ROOM}")).await;

    while let Ok(Some(line)) = lines.next_line().await {
        let parsed: ClientMessage = match serde_json::from_str(&line) {
            Ok(msg) => msg,
            Err(_) => {
                println!("Invalid JSON from {username}");
                continue;
            }
        };

        match parsed {
            ClientMessage::Chat { message } => {
                let message = message.trim();

                if message.is_empty(){
                    continue;
                }

                if let Some(room) = get_client_room(&clients, client_id).await {
                    println!("[{room}] {username}: {message}");
                    broadcast_chat_to_room(&clients, &username, &room, message).await;
                }
            }
            ClientMessage::SetUsername { .. } => {
                println!("{username} tried to change username mid-session");
            }
            ClientMessage::JoinRoom { room } => {
                let room = room.trim();

                if room.is_empty(){
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
                            &clients, &old_room, &format!("{username} left {old_room}"),
                        ).await;

                        broadcast_system_to_room(
                            &clients, room, &format!("{username} joined {room}"),
                        ).await;
                    }
                }
            }
            ClientMessage::ListRooms => {
                let rooms = list_rooms(&clients).await;

                let _ = {
                    let clients_guard = clients.lock().await;
                    clients_guard.get(&client_id).map(|client| {
                        client.tx.send(ServerMessage::RoomList { rooms })
                    })
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
                }
            }
        }
    }

    let current_room = get_client_room(&clients, client_id).await.unwrap_or_else(|| DEFAULT_ROOM.to_string());

    {
        let mut clients_guard = clients.lock().await;
        clients_guard.remove(&client_id);
    }

    broadcast_system_to_room(&clients, &current_room, &format!("{username} left {current_room}")).await;
    println!("{username} disconnected");
}