use std::io::{Read, Write};

use serde::Serialize;
use serde::de::DeserializeOwned;
use thiserror::Error;

pub use crate::tui::protocol::*;
pub use client::*;

mod client;

pub const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;

pub fn write_frame<W: Write, T: Serialize>(writer: &mut W, value: &T) -> Result<(), FrameError> {
    let encoded = serde_json::to_vec(value).map_err(FrameError::Encode)?;
    if encoded.len() > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge(encoded.len()));
    }
    let length = u32::try_from(encoded.len()).map_err(|_| FrameError::TooLarge(encoded.len()))?;
    writer
        .write_all(&length.to_be_bytes())
        .map_err(FrameError::Io)?;
    writer.write_all(&encoded).map_err(FrameError::Io)?;
    writer.flush().map_err(FrameError::Io)
}

pub fn read_frame<R: Read, T: DeserializeOwned>(reader: &mut R) -> Result<T, FrameError> {
    let mut prefix = [0_u8; 4];
    reader.read_exact(&mut prefix).map_err(FrameError::Io)?;
    let length = u32::from_be_bytes(prefix) as usize;
    if length == 0 {
        return Err(FrameError::Empty);
    }
    if length > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge(length));
    }
    let mut encoded = vec![0_u8; length];
    reader.read_exact(&mut encoded).map_err(FrameError::Io)?;
    serde_json::from_slice(&encoded).map_err(FrameError::Decode)
}

#[derive(Debug, Error)]
pub enum FrameError {
    #[error("IPC frame I/O failed: {0}")]
    Io(#[source] std::io::Error),
    #[error("cannot encode IPC frame: {0}")]
    Encode(#[source] serde_json::Error),
    #[error("cannot decode IPC frame: {0}")]
    Decode(#[source] serde_json::Error),
    #[error("IPC frame is empty")]
    Empty,
    #[error("IPC frame is {0} bytes, exceeding the limit")]
    TooLarge(usize),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_round_trip_is_length_prefixed() {
        let mut encoded = Vec::new();
        write_frame(&mut encoded, &serde_json::json!({"ok": true})).unwrap();
        assert_eq!(u32::from_be_bytes(encoded[..4].try_into().unwrap()), 11);

        let decoded: serde_json::Value = read_frame(&mut encoded.as_slice()).unwrap();
        assert_eq!(decoded, serde_json::json!({"ok": true}));
    }

    #[test]
    fn oversized_prefix_is_rejected_before_allocation() {
        let prefix = u32::try_from(MAX_FRAME_BYTES + 1).unwrap().to_be_bytes();
        let error = read_frame::<_, serde_json::Value>(&mut prefix.as_slice()).unwrap_err();
        assert!(matches!(error, FrameError::TooLarge(_)));
    }
}
