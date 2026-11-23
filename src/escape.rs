//! Control characters out of text the run did not author (docs/spec/output.md OUT-12).
//!
//! A mirror URL, its `Origin`, `Label` or error text, and any error or warning that quotes one,
//! can carry `ESC`. Printing that as root into a terminal the unprivileged user shares is a control-sequence
//! injection into their terminal (`docs/threat-model.md` B7), so every such string is rewritten before it is
//! printed: a control character becomes `\\x` and two lowercase hex digits. The run own escapes stay: colour
//! and the progress line are the run output, and this module is only about bytes that come from outside it.

/// Rewrite every control character as `\\x` plus two lowercase hex digits.
///
/// Every control character is rewritten, Unicode `Cc` included: `ESC` and the 8-bit C1 range a terminal
/// may still honour. Text with nothing to rewrite is returned unchanged.
pub fn control_free(text: &str) -> String {
    if !text.chars().any(char::is_control) {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + 8);
    for ch in text.chars() {
        if ch.is_control() {
            out.push_str("\\x");
            out.push(hex(((ch as u32) >> 4) as u8));
            out.push(hex(((ch as u32) & 0x0f) as u8));
        } else {
            out.push(ch);
        }
    }
    out
}

fn hex(nibble: u8) -> char {
    match nibble {
        0..=9 => (b'0' + nibble) as char,
        _ => (b'a' + nibble - 10) as char,
    }
}

#[cfg(test)]
mod tests {
    use super::control_free;

    #[test]
    fn text_without_control_characters_is_unchanged() {
        let url = "http://mirror.example/ubuntu";
        assert_eq!(control_free(url), url);
    }

    #[test]
    fn an_escape_becomes_its_visible_spelling() {
        assert_eq!(control_free("\u{1b}[31mred"), "\\x1b[31mred");
    }

    #[test]
    fn every_ascii_control_character_is_rewritten_including_del() {
        for byte in 0u8..=0x1f {
            let input = String::from_utf8_lossy(&[byte]).to_string();
            let out = control_free(&input);
            assert_eq!(out, format!("\\x{:02x}", byte), "byte {byte}");
            assert!(!out.chars().any(char::is_control), "byte {byte} leaked");
        }
        assert_eq!(control_free("\u{7f}"), "\\x7f");
    }

    #[test]
    fn a_non_ascii_character_is_not_a_control_one() {
        assert_eq!(control_free("m\u{fc}nchen"), "m\u{fc}nchen");
    }
}
