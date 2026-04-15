use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClientMessage {
    #[serde(rename = "register")]
    Register { username: String, password: String },

    #[serde(rename = "login")]
    Login { username: String, password: String },

    #[serde(rename = "chat")]
    Chat { message: String },

    #[serde(rename = "join_room")]
    JoinRoom { room: String },

    #[serde(rename = "leave_room")]
    LeaveRoom,

    #[serde(rename = "list_rooms")]
    ListRooms,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ServerMessage {
    #[serde(rename = "auth_ok")]
    AuthOk,

    #[serde(rename = "auth_error")]
    AuthError { message: String },

    #[serde(rename = "welcome")]
    Welcome { message: String },

    #[serde(rename = "error")]
    Error { message: String },

    #[serde(rename = "system")]
    System { message: String },

    #[serde(rename = "chat")]
    Chat {
        username: String,
        room: String,
        message: String,
    },

    #[serde(rename = "room_joined")]
    RoomJoined { room: String },

    #[serde(rename = "room_list")]
    RoomList { rooms: Vec<String> },
}
