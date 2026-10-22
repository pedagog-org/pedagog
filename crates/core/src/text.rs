/// The end of `s`, at most `max` bytes, cut on a character boundary (a byte slice
/// would panic inside a multi-byte character).
pub fn tail(s: &str, max: usize) -> &str {
    &s[s.ceil_char_boundary(s.len().saturating_sub(max))..]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cut_inside_a_character_keeps_less_rather_than_panicking() {
        // 'é' is two bytes: the last 2 bytes are "é", the last 3 would start inside "ü".
        assert_eq!(tail("aüé", 2), "é");
        assert_eq!(tail("aüé", 3), "é");
        assert_eq!(tail("aüé", 4), "üé");
    }

    #[test]
    fn short_text_is_kept_whole() {
        assert_eq!(tail("log", 4000), "log");
        assert_eq!(tail("", 10), "");
    }
}
