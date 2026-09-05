//! LSP Content-Length framing.
//!
//! The body is not interpreted as JSON; it is read and written as a byte sequence
//! (v0.1-design.md 4.6: no full parse + re-serialization).

use std::io::{self, BufRead, Write};

#[derive(Debug, thiserror::Error)]
pub enum FramingError {
    #[error("i/o error")]
    Io(#[from] io::Error),
    #[error("malformed header line (missing `\\r\\n` terminator): {0:?}")]
    MalformedHeaderLine(String),
    #[error("malformed header (missing `: ` separator): {0:?}")]
    MalformedHeader(String),
    #[error("missing required header content-length")]
    MissingContentLength,
    #[error("invalid content-length value: {0:?}")]
    InvalidContentLength(String),
}

/// The body of one message. Only the Content-Length header is handled
/// (other headers such as Content-Type are read and discarded, and not reconstructed on write).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawMessage {
    pub body: Vec<u8>,
}

/// Reads one message from the stream.
/// Returns `Ok(None)` on a clean EOF (cut off at the start of a header).
pub fn read_message<R: BufRead>(reader: &mut R) -> Result<Option<RawMessage>, FramingError> {
    let content_length = match read_header(reader)? {
        Some(len) => len,
        None => return Ok(None),
    };

    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body)?;
    Ok(Some(RawMessage { body }))
}

/// Reads the header part and returns the Content-Length.
/// `Ok(None)` when EOF is reached before the first byte of a header is read
/// (a clean disconnect). EOF in the middle of a header is an `Io` error
/// (`read_line` returns `UnexpectedEof`).
fn read_header<R: BufRead>(reader: &mut R) -> Result<Option<usize>, FramingError> {
    let mut content_length: Option<usize> = None;
    let mut line = String::new();
    let mut at_start = true;

    loop {
        line.clear();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            if at_start {
                return Ok(None);
            }
            return Err(FramingError::MalformedHeaderLine(line));
        }
        at_start = false;

        let text = line
            .strip_suffix("\r\n")
            .ok_or_else(|| FramingError::MalformedHeaderLine(line.clone()))?;

        if text.is_empty() {
            // The empty line separating the header from the body.
            break;
        }

        let (name, value) = text
            .split_once(": ")
            .ok_or_else(|| FramingError::MalformedHeader(text.to_string()))?;

        if name.eq_ignore_ascii_case("content-length") {
            let len = value
                .parse::<usize>()
                .map_err(|_| FramingError::InvalidContentLength(value.to_string()))?;
            content_length = Some(len);
        }
        // content-type and other unknown headers are leniently read and discarded.
    }

    content_length
        .ok_or(FramingError::MissingContentLength)
        .map(Some)
}

/// Writes one message to the stream. Content-Length is recomputed from body.len().
pub fn write_message<W: Write>(writer: &mut W, msg: &RawMessage) -> io::Result<()> {
    write!(writer, "Content-Length: {}\r\n\r\n", msg.body.len())?;
    writer.write_all(&msg.body)?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufReader, Cursor, Read};

    fn framed(body: &str) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes());
        out.extend_from_slice(body.as_bytes());
        out
    }

    #[test]
    fn reads_a_single_message() {
        let input = framed(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#);
        let mut reader = BufReader::new(Cursor::new(input));
        let msg = read_message(&mut reader).unwrap().unwrap();
        assert_eq!(
            msg.body,
            br#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#
        );
    }

    #[test]
    fn reads_multiple_sequential_messages_in_order() {
        let mut input = framed(r#"{"n":1}"#);
        input.extend_from_slice(&framed(r#"{"n":2}"#));
        let mut reader = BufReader::new(Cursor::new(input));

        let first = read_message(&mut reader).unwrap().unwrap();
        assert_eq!(first.body, br#"{"n":1}"#);
        let second = read_message(&mut reader).unwrap().unwrap();
        assert_eq!(second.body, br#"{"n":2}"#);
    }

    #[test]
    fn returns_none_on_clean_eof_before_any_header() {
        let mut reader = BufReader::new(Cursor::new(Vec::<u8>::new()));
        assert!(read_message(&mut reader).unwrap().is_none());
    }

    #[test]
    fn errors_on_missing_content_length() {
        let input = b"Content-Type: application/json\r\n\r\n".to_vec();
        let mut reader = BufReader::new(Cursor::new(input));
        let err = read_message(&mut reader).unwrap_err();
        assert!(matches!(err, FramingError::MissingContentLength));
    }

    #[test]
    fn ignores_unknown_headers() {
        // Must not break if a server adds an unknown header in the future (lenient reading).
        let mut input = b"X-Future-Header: value\r\n".to_vec();
        input.extend_from_slice(format!("Content-Length: {}\r\n\r\n", 2).as_bytes());
        input.extend_from_slice(b"{}");
        let mut reader = BufReader::new(Cursor::new(input));
        let msg = read_message(&mut reader).unwrap().unwrap();
        assert_eq!(msg.body, b"{}");
    }

    /// Simulation of fragmented arrival: the header and body must be read correctly even with
    /// a Reader that returns only one byte at a time (equivalent to ADR 0005 checklist #4).
    struct OneByteAtATimeReader {
        data: Vec<u8>,
        pos: usize,
    }

    impl Read for OneByteAtATimeReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.pos >= self.data.len() {
                return Ok(0);
            }
            buf[0] = self.data[self.pos];
            self.pos += 1;
            Ok(1)
        }
    }

    #[test]
    fn reads_message_split_across_many_tiny_reads() {
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
        let input = framed(body);
        let mut reader = BufReader::new(OneByteAtATimeReader {
            data: input,
            pos: 0,
        });
        let msg = read_message(&mut reader).unwrap().unwrap();
        assert_eq!(msg.body, body.as_bytes());
    }

    #[test]
    fn reads_large_message_split_across_tiny_reads() {
        // Regression test for the real case reported in ls_proxy: "parsing breaks when a huge
        // message is fragmented".
        let large_body = format!(r#"{{"data":"{}"}}"#, "x".repeat(500_000));
        let input = framed(&large_body);
        let mut reader = BufReader::new(OneByteAtATimeReader {
            data: input,
            pos: 0,
        });
        let msg = read_message(&mut reader).unwrap().unwrap();
        assert_eq!(msg.body, large_body.as_bytes());
    }

    #[test]
    fn write_then_read_roundtrips_bytes_exactly() {
        // Verifies that no full parse + re-serialization happens.
        // A naive JSON structure cannot detect that key order and number notation ("1.0" etc.)
        // are unchanged, so here non-destructive forwarding is guaranteed by "the written bytes
        // can be read back exactly as they are".
        let body = br#"{"z":1,"a":2.0,"m":"x"}"#.to_vec();
        let mut buf = Vec::new();
        write_message(&mut buf, &RawMessage { body: body.clone() }).unwrap();

        let mut reader = BufReader::new(Cursor::new(buf));
        let msg = read_message(&mut reader).unwrap().unwrap();
        assert_eq!(msg.body, body);
    }

    #[test]
    fn write_message_uses_correct_content_length_header() {
        let body = b"{}".to_vec();
        let mut buf = Vec::new();
        write_message(&mut buf, &RawMessage { body }).unwrap();
        assert_eq!(buf, b"Content-Length: 2\r\n\r\n{}");
    }
}
