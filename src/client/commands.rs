use crate::client::app::App;
use crate::client::network::send_json;
use crate::shared::protocol::ClientMessage;
use tokio::net::tcp::OwnedWriteHalf;

pub async fn handle_input(app: &mut App, writer: &mut OwnedWriteHalf) {
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
