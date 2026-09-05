//! Conversion between local paths and `file:` URIs (used by the downstream side's stand-in.
//! ADR 0015).
//!
//! Only the `file:` scheme is handled, and there is no host name. Every byte of the path is
//! percent-encoded except RFC 3986 unreserved characters, `/`, and `:` (the Windows drive
//! letter). A Windows drive takes the form `file:///C:/...`, and `\` becomes `/`.

use std::path::{Path, PathBuf};

/// Turns a path into a `file:` URI.
pub fn path_to_uri(path: &Path) -> String {
    let text = path.to_string_lossy();
    let mut out = String::from("file://");
    let normalized: String = if cfg!(windows) {
        text.replace('\\', "/")
    } else {
        text.into_owned()
    };
    // A Windows drive (`C:/...`) becomes `file:///C:/...`.
    if !normalized.starts_with('/') {
        out.push('/');
    }
    for byte in normalized.bytes() {
        if is_unreserved(byte) || byte == b'/' || byte == b':' {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Turns a `file:` URI into a path. Other schemes and unreadable forms give `None`.
pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    // `file:///path` has no host. `file://host/path` is not handled.
    let path = rest
        .strip_prefix('/')
        .map(|p| format!("/{p}"))
        .or_else(|| rest.is_empty().then(String::new))?;
    let decoded = percent_decode(&path);
    if cfg!(windows) {
        // `/C:/...` -> `C:/...`.
        let trimmed = decoded
            .strip_prefix('/')
            .filter(|p| p.as_bytes().get(1) == Some(&b':'))
            .map(str::to_string)
            .unwrap_or(decoded);
        Some(PathBuf::from(trimmed.replace('/', "\\")))
    } else {
        Some(PathBuf::from(decoded))
    }
}

fn is_unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = &text[i + 1..i + 3];
            if let Ok(value) = u8::from_str_radix(hex, 16) {
                out.push(value);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_spaces_and_keeps_slashes() {
        let uri = path_to_uri(Path::new(if cfg!(windows) {
            r"C:\w s\a.rs"
        } else {
            "/w s/a.rs"
        }));
        assert_eq!(
            uri,
            if cfg!(windows) {
                "file:///C:/w%20s/a.rs"
            } else {
                "file:///w%20s/a.rs"
            }
        );
    }

    #[test]
    fn round_trips_a_path() {
        let path = PathBuf::from(if cfg!(windows) {
            r"C:\work\src\lib.rs"
        } else {
            "/work/src/lib.rs"
        });
        assert_eq!(uri_to_path(&path_to_uri(&path)), Some(path));
    }

    #[test]
    fn rejects_other_schemes() {
        assert_eq!(uri_to_path("untitled:Untitled-1"), None);
    }
}
