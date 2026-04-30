//! Frame codec for the daemon ↔ picker-ui Unix socket.
//!
//! Wire format: 4-byte big-endian unsigned length prefix, then UTF-8
//! JSON body. Mirrors the notification-daemon broadcast protocol so
//! contributors and reviewers do not have to learn a new shape.
//!
//! These functions are deliberately sync — they touch buffers, not the
//! network. The daemon and picker-ui each wrap them in async tokio I/O.
//!
//! Frame size is bounded so a malicious peer cannot stall the daemon by
//! announcing a 4 GiB message and then dribbling bytes. The cap is
//! generous (1 MiB) for legitimate traffic — even a `multiple=true`
//! pick with 256 long path entries fits comfortably — but rejects the
//! pathological cases.

use std::convert::TryInto;

use serde::{de::DeserializeOwned, Serialize};

/// Hard cap on a single frame body, in bytes. 1 MiB is well above the
/// realistic upper bound for picker traffic and well below anything
/// that would let a peer DoS the daemon. See edge case E22 for the
/// related multi-select size discussion (the picker UI applies its own
/// soft cap before we hit this).
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("frame body exceeds the {MAX_FRAME_BYTES}-byte cap (got {got})")]
    FrameTooLarge { got: usize },
    #[error("frame body is zero-length")]
    EmptyFrame,
    #[error("JSON serialization failed: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("incomplete frame: need {need} bytes, have {have}")]
    Incomplete { need: usize, have: usize },
}

/// Serialize `msg` as a length-prefixed JSON frame ready to be written
/// to the socket. The returned `Vec` owns its memory; if you need to
/// avoid the allocation in a hot path, write the prefix and the body
/// in two separate writes (see `decode_frame` for the matching
/// read-side that also avoids reallocation).
pub fn encode_frame<T: Serialize>(msg: &T) -> Result<Vec<u8>, CodecError> {
    let body = serde_json::to_vec(msg)?;
    if body.is_empty() {
        return Err(CodecError::EmptyFrame);
    }
    if body.len() > MAX_FRAME_BYTES {
        return Err(CodecError::FrameTooLarge { got: body.len() });
    }
    let mut out = Vec::with_capacity(4 + body.len());
    let len = u32::try_from(body.len()).expect("checked above");
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

/// Try to decode a single frame from the front of `buf`.
///
/// Returns the number of bytes consumed plus the decoded message on
/// success. The caller is expected to advance its read buffer by the
/// returned count. On `Incomplete`, the caller should read more bytes
/// and retry — the buffer state is unchanged.
///
/// On other errors the buffer state is also unchanged but the
/// connection should generally be dropped, since a malformed frame at
/// this point means the peer is broken or hostile.
pub fn decode_frame<T: DeserializeOwned>(buf: &[u8]) -> Result<(usize, T), CodecError> {
    if buf.len() < 4 {
        return Err(CodecError::Incomplete {
            need: 4,
            have: buf.len(),
        });
    }
    let len = u32::from_be_bytes(buf[..4].try_into().expect("checked above")) as usize;
    if len == 0 {
        return Err(CodecError::EmptyFrame);
    }
    if len > MAX_FRAME_BYTES {
        return Err(CodecError::FrameTooLarge { got: len });
    }
    if buf.len() < 4 + len {
        return Err(CodecError::Incomplete {
            need: 4 + len,
            have: buf.len(),
        });
    }
    let msg: T = serde_json::from_slice(&buf[4..4 + len])?;
    Ok((4 + len, msg))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PickerResponse;
    use std::path::PathBuf;

    /// Round-trip: encoded frame plus a trailing byte decodes back the
    /// original and reports the correct consumed-byte count.
    #[test]
    fn round_trip_with_trailing_bytes() {
        let resp = PickerResponse::Picked {
            handle: "h1".into(),
            paths: vec![PathBuf::from("/home/user/file.txt")],
            current_filter: None,
        };
        let mut wire = encode_frame(&resp).unwrap();
        // Garbage byte after the frame to verify `consumed` is exact.
        wire.push(0xFF);
        let (consumed, decoded): (usize, PickerResponse) = decode_frame(&wire).unwrap();
        assert_eq!(consumed, wire.len() - 1);
        assert_eq!(format!("{resp:?}"), format!("{decoded:?}"));
    }

    /// Decoding a buffer that is too short to even hold the length
    /// prefix returns `Incomplete` rather than panicking.
    #[test]
    fn incomplete_too_short_for_length() {
        let buf = [0u8, 1, 2];
        let result: Result<(usize, PickerResponse), _> = decode_frame(&buf);
        assert!(matches!(result, Err(CodecError::Incomplete { .. })));
    }

    /// Decoding a length prefix that promises more bytes than the
    /// buffer holds returns `Incomplete` with the missing count.
    #[test]
    fn incomplete_body_partial() {
        let mut buf = vec![0u8, 0, 0, 100];
        buf.extend_from_slice(b"{"); // 1 byte of body, need 100
        let result: Result<(usize, PickerResponse), _> = decode_frame(&buf);
        match result {
            Err(CodecError::Incomplete { need, have }) => {
                assert_eq!(need, 4 + 100);
                assert_eq!(have, 5);
            }
            other => panic!("expected Incomplete, got {other:?}"),
        }
    }

    /// Frame-too-large guards both sides: encode rejects oversized
    /// bodies and decode rejects oversized prefixes (so a hostile peer
    /// cannot make us allocate gigabytes by claiming a huge length).
    #[test]
    fn frame_too_large_decode() {
        // Length prefix says 2 GiB.
        let buf = vec![0x80, 0, 0, 0];
        let result: Result<(usize, PickerResponse), _> = decode_frame(&buf);
        assert!(matches!(result, Err(CodecError::FrameTooLarge { .. })));
    }

    /// Zero-length frame is rejected explicitly so a peer cannot block
    /// us by sending a stream of empty headers.
    #[test]
    fn empty_frame_decode() {
        let buf = vec![0u8, 0, 0, 0];
        let result: Result<(usize, PickerResponse), _> = decode_frame(&buf);
        assert!(matches!(result, Err(CodecError::EmptyFrame)));
    }
}
