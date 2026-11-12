//! Newline-delimited framing with a hard size limit (std-only).
//!
//! Every frame is one line. JSON produced by `serde_json::to_vec` never
//! contains a raw newline, so no escaping layer is needed.

use crate::error::{Error, Result};
use std::io::{self, BufRead, Write};

pub fn write_frame<W: Write>(w: &mut W, payload: &[u8]) -> Result<()> {
    if payload.contains(&b'\n') {
        return Err(Error::Ipc("payload contains a newline".into()));
    }
    w.write_all(payload)?;
    w.write_all(b"\n")?;
    w.flush()?;
    Ok(())
}

/// Reads one frame. `Ok(None)` means clean end of stream.
/// Never buffers more than `max` bytes of a single frame.
pub fn read_frame<R: BufRead>(r: &mut R, max: usize) -> Result<Option<Vec<u8>>> {
    let mut out: Vec<u8> = Vec::new();
    loop {
        let buf = match r.fill_buf() {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        };
        if buf.is_empty() {
            return if out.is_empty() {
                Ok(None)
            } else {
                Err(Error::Ipc("truncated frame".into()))
            };
        }
        match buf.iter().position(|&b| b == b'\n') {
            Some(i) => {
                if out.len() + i > max {
                    return Err(Error::Ipc("frame too large".into()));
                }
                out.extend_from_slice(&buf[..i]);
                r.consume(i + 1);
                return Ok(Some(out));
            }
            None => {
                let n = buf.len();
                if out.len() + n > max {
                    return Err(Error::Ipc("frame too large".into()));
                }
                out.extend_from_slice(buf);
                r.consume(n);
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::io::{BufReader, Cursor};

    #[test]
    fn round_trip_two_frames() {
        let mut wire = Vec::new();
        write_frame(&mut wire, b"{\"a\":1}").unwrap();
        write_frame(&mut wire, b"second").unwrap();
        let mut r = Cursor::new(wire);
        assert_eq!(read_frame(&mut r, 64).unwrap().unwrap(), b"{\"a\":1}");
        assert_eq!(read_frame(&mut r, 64).unwrap().unwrap(), b"second");
        assert!(read_frame(&mut r, 64).unwrap().is_none());
    }

    #[test]
    fn rejects_oversized_frame_even_across_buffer_refills() {
        let data = vec![b'x'; 100];
        let mut r = BufReader::with_capacity(8, Cursor::new(data));
        assert!(read_frame(&mut r, 32).is_err());
    }

    #[test]
    fn truncated_frame_is_an_error() {
        let mut r = Cursor::new(b"no newline".to_vec());
        assert!(read_frame(&mut r, 64).is_err());
    }

    #[test]
    fn payload_with_newline_is_refused() {
        let mut sink = Vec::new();
        assert!(write_frame(&mut sink, b"a\nb").is_err());
    }
}
