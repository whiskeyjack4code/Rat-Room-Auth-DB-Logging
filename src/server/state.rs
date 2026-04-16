use argon2::{
    Argon2,
    password_hash::{PasswordHasher, SaltString, rand_core::OsRng},
};
use sqlx::{Row, SqlitePool};

pub async fn create_user(
    db: &SqlitePool,
    username: &str,
    password: &str,
) -> Result<(), sqlx::Error> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();

    let password_hash = argon2
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| sqlx::Error::Protocol(format!("argon2 hash error: {e}").into()))?
        .to_string();

    sqlx::query("INSERT INTO users (username, password) VALUES (?, ?)")
        .bind(username)
        .bind(password_hash)
        .execute(db)
        .await?;

    Ok(())
}

pub async fn get_password_hash(
    db: &SqlitePool,
    username: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row = sqlx::query("SELECT password FROM users WHERE username = ?")
        .bind(username)
        .fetch_optional(db)
        .await?;

    Ok(row.map(|r| r.get("password")))
}

pub async fn save_message(
    db: &SqlitePool,
    username: &str,
    room: &str,
    message: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO messages (username, room, message, created_at)
         VALUES (?, ?, ?, datetime('now'))",
    )
    .bind(username)
    .bind(room)
    .bind(message)
    .execute(db)
    .await?;

    Ok(())
}
