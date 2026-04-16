mod server;
mod shared;

use server::client_handler::handle_client;
use shared::protocol::*;

use sqlx::SqlitePool;
use sqlx::sqlite::SqliteConnectOptions;
use std::str::FromStr;
use tracing::{error, info};
use tracing_subscriber::{EnvFilter, fmt};

use std::collections::HashMap;
use std::sync::{Arc, atomic::AtomicUsize};

use serde::Deserialize;
use std::fs;
use tokio::net::TcpListener;
use tokio::sync::{Mutex, mpsc};

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

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS messages (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            username TEXT NOT NULL,
            room TEXT NOT NULL,
            message TEXT NOT NULL,
            created_at TEXT NOT NULL
        )",
    )
    .execute(&db)
    .await
    .expect("Failed to create messages table");

    info!("database connected and messages table ready");

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
