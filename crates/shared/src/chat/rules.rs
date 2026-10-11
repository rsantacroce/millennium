//! Message validation applied by the server to every incoming chat message.

/// Maximum message length, in characters.
pub const MAX_CHAT_LEN: usize = 255;
/// Say chat reaches players within this many meters (same zone only).
pub const SAY_RANGE: f32 = 20.0;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RejectReason {
    #[error("message is empty")]
    Empty,
    #[error("message is too long (max {MAX_CHAT_LEN} characters)")]
    TooLong,
}

/// Trims, strips control characters (newlines, escape codes, zero-width tricks) and
/// collapses runs of whitespace. Rejects empty or over-long results.
pub fn sanitize(text: &str) -> Result<String, RejectReason> {
    let mut out = String::with_capacity(text.len());
    let mut last_space = true; // also trims leading whitespace
    for c in text.chars() {
        if c.is_whitespace() {
            if !last_space {
                out.push(' ');
                last_space = true;
            }
        } else if !c.is_control() && !is_invisible(c) {
            out.push(c);
            last_space = false;
        }
    }
    let out = out.trim_end().to_string();
    if out.is_empty() {
        return Err(RejectReason::Empty);
    }
    if out.chars().count() > MAX_CHAT_LEN {
        return Err(RejectReason::TooLong);
    }
    Ok(out)
}

fn is_invisible(c: char) -> bool {
    matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2064}' | '\u{FEFF}')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_and_collapses() {
        assert_eq!(sanitize("  hello \n\t  world  "), Ok("hello world".into()));
    }

    #[test]
    fn strips_control_and_invisible() {
        assert_eq!(sanitize("a\u{7}b\u{200B}c\u{202E}d"), Ok("abcd".into()));
    }

    #[test]
    fn rejects_empty_and_long() {
        assert_eq!(sanitize(" \u{200B} "), Err(RejectReason::Empty));
        assert_eq!(sanitize(&"x".repeat(MAX_CHAT_LEN)).map(|s| s.len()), Ok(MAX_CHAT_LEN));
        assert_eq!(sanitize(&"x".repeat(MAX_CHAT_LEN + 1)), Err(RejectReason::TooLong));
        // Length counts characters, not bytes.
        assert!(sanitize(&"é".repeat(MAX_CHAT_LEN)).is_ok());
    }
}
