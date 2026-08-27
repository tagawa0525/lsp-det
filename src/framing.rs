//! LSP の Content-Length フレーミング。
//!
//! ボディは JSON として解釈せず、バイト列のまま読み書きする
//! (v0.1-design.md 4.6: 完全パース+再シリアライズ禁止)。

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

/// 1 メッセージのボディ。ヘッダは Content-Length のみを扱う
/// (Content-Type 等の他ヘッダは読み捨て、書き込み時は再構成しない)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawMessage {
    pub body: Vec<u8>,
}

/// ストリームから 1 メッセージを読む。
/// クリーンな EOF (ヘッダの先頭で切れている) なら `Ok(None)` を返す。
pub fn read_message<R: BufRead>(reader: &mut R) -> Result<Option<RawMessage>, FramingError> {
    let content_length = match read_header(reader)? {
        Some(len) => len,
        None => return Ok(None),
    };

    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body)?;
    Ok(Some(RawMessage { body }))
}

/// ヘッダ部を読み、Content-Length を返す。
/// ヘッダの先頭バイトを読む前に EOF に達した場合は `Ok(None)`
/// (クリーンな切断)。ヘッダの途中で EOF に達した場合は `Io` エラーになる
/// (`read_line` が `UnexpectedEof` を返す)。
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
            // ヘッダとボディを区切る空行。
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
        // content-type やその他の未知ヘッダは寛容に読み捨てる。
    }

    content_length
        .ok_or(FramingError::MissingContentLength)
        .map(Some)
}

/// ストリームへ 1 メッセージを書く。Content-Length は body.len() から再計算する。
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
        // 将来サーバーが未知のヘッダを追加しても壊れないこと (寛容な読み取り)。
        let mut input = b"X-Future-Header: value\r\n".to_vec();
        input.extend_from_slice(format!("Content-Length: {}\r\n\r\n", 2).as_bytes());
        input.extend_from_slice(b"{}");
        let mut reader = BufReader::new(Cursor::new(input));
        let msg = read_message(&mut reader).unwrap().unwrap();
        assert_eq!(msg.body, b"{}");
    }

    /// 分割到着のシミュレーション: 1 バイトずつしか返さない Reader でも
    /// ヘッダ・ボディを正しく読めること (ADR 0005 チェックリスト #4 相当)。
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
        // ls_proxy で報告された「巨大メッセージの分割でパースが壊れる」実例への回帰テスト。
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
        // 完全パース+再シリアライズをしていないことの検証。
        // キー順序・数値表記("1.0"等)が変化しないことを、素朴なJSON構造では
        // 検出できないため、ここでは「書いたバイト列がそのまま読み返せる」ことで
        // 非破壊転送を保証する。
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
