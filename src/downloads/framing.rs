//! Chromium native messaging, with the helper's 1 MiB response limit.

use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const MAX_MESSAGE: usize = 1024 * 1024;

pub(super) async fn read(reader: &mut (impl AsyncRead + Unpin)) -> Result<Value, String> {
    let size = reader
        .read_u32_le()
        .await
        .map_err(|_| "The Siphon helper closed its connection.".to_string())?
        as usize;
    if !(1..=MAX_MESSAGE).contains(&size) {
        return Err("The Siphon helper sent an invalid message length.".into());
    }
    let mut bytes = vec![0; size];
    reader
        .read_exact(&mut bytes)
        .await
        .map_err(|_| "The Siphon helper sent an incomplete message.".to_string())?;
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| "The Siphon helper sent invalid JSON.".to_string())?;
    if !value.is_object() {
        return Err("The Siphon helper sent an invalid message.".into());
    }
    Ok(value)
}

pub(super) async fn write(
    writer: &mut (impl AsyncWrite + Unpin),
    value: &Value,
) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    if !value.is_object() || bytes.len() > MAX_MESSAGE {
        return Err("Invalid Siphon helper request.".into());
    }
    writer
        .write_u32_le(bytes.len() as u32)
        .await
        .map_err(|error| error.to_string())?;
    writer.write_all(&bytes).await.map_err(|e| e.to_string())?;
    writer.flush().await.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn frames_are_little_endian_utf8_and_flush_each_message() {
        let message = json!({"id": "one", "title": "Björk 音楽"});
        let mut bytes = Vec::new();
        write(&mut bytes, &message).await.unwrap();
        assert_eq!(
            u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize,
            bytes.len() - 4
        );
        let mut stream = bytes.as_slice();
        assert_eq!(read(&mut stream).await.unwrap(), message);
        assert!(read(&mut stream).await.is_err());
    }

    #[tokio::test]
    async fn rejects_truncation_empty_oversized_and_nonobject_frames() {
        for bytes in [
            vec![],
            vec![3, 0],
            vec![0, 0, 0, 0],
            ((MAX_MESSAGE + 1) as u32).to_le_bytes().to_vec(),
            vec![5, 0, 0, 0, b'{'],
            vec![2, 0, 0, 0, b'[', b']'],
            vec![1, 0, 0, 0, 0xff],
        ] {
            assert!(read(&mut bytes.as_slice()).await.is_err(), "{bytes:?}");
        }
    }

    #[tokio::test]
    async fn fragmented_and_consecutive_frames_remain_separate() {
        let (mut writer, mut reader) = tokio::io::duplex(1);
        let sending = tokio::spawn(async move {
            write(&mut writer, &json!({"id": "1"})).await.unwrap();
            write(&mut writer, &json!({"id": "2"})).await.unwrap();
        });
        assert_eq!(read(&mut reader).await.unwrap()["id"], "1");
        assert_eq!(read(&mut reader).await.unwrap()["id"], "2");
        sending.await.unwrap();
    }
}
