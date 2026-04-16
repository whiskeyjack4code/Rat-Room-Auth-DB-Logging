use crate::shared::protocol::ClientMessage;
use tokio::io::AsyncWriteExt;
use tokio::net::tcp::OwnedWriteHalf;

pub async fn send_json(writer: &mut OwnedWriteHalf, message: &ClientMessage) {
    if let Ok(json) = serde_json::to_string(message) {
        let _ = writer.write_all(json.as_bytes()).await;
        let _ = writer.write_all(b"\n").await;
    }
}
