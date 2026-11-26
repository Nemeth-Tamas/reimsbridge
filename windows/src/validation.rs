use std::error::Error;

pub const MAX_CLIPBOARD_BYTES: usize = 1024 * 1024;

pub fn validate_text(text: &str) -> Result<(), Box<dyn Error>> {
    if text.is_empty() {
        return Err("Clipboard contains no text".into());
    }

    if text.len() > MAX_CLIPBOARD_BYTES {
        return Err("Clipboard exceeds size limit".into());
    }

    if text.contains('\0') {
        return Err("Clipboard contains embedded NUL".into());
    }

    Ok(())
}

pub fn decode_text(bytes: Vec<u8>) -> Result<String, Box<dyn Error>> {
    if bytes.len() > MAX_CLIPBOARD_BYTES {
        return Err("Clipboard exceeds size limit".into());
    }

    let text = String::from_utf8(bytes)?;

    validate_text(&text)?;

    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_text() {
        assert!(validate_text("Hello world").is_ok());
    }

    #[test]
    fn unicode_text() {
        assert!(validate_text("Árvíztűrő tükörfúrógép 🚀").is_ok());
    }

    #[test]
    fn empty_text_rejected() {
        assert!(validate_text("").is_err());
    }

    #[test]
    fn embedded_nul_rejected() {
        assert!(validate_text("Hello\0World").is_err());
    }

    #[test]
    fn whitespace_preserved() {
        let text = " \t\n\r\n    ";
        assert_eq!(decode_text(text.as_bytes().to_vec()).unwrap(), text);
    }

    #[test]
    fn mixed_line_endings_preserved() {
        let text = "LF\nCRLF\r\nCR\rLast line\n";
        assert_eq!(decode_text(text.as_bytes().to_vec()).unwrap(), text);
    }

    #[test]
    fn exact_size_limit_accepted() {
        let text = "x".repeat(MAX_CLIPBOARD_BYTES);
        assert!(validate_text(&text).is_ok());
    }

    #[test]
    fn oversized_text_rejected() {
        let text = "x".repeat(MAX_CLIPBOARD_BYTES + 1);
        assert!(validate_text(&text).is_err());
    }

    #[test]
    fn size_limit_counts_bytes() {
        let text = "🚀".repeat(MAX_CLIPBOARD_BYTES / 4);

        assert_eq!(text.len(), MAX_CLIPBOARD_BYTES);
        assert!(validate_text(&text).is_ok());

        let oversized = format!("{text}x");
        assert!(validate_text(&oversized).is_err());
    }

    #[test]
    fn invalid_utf8_rejected() {
        assert!(decode_text(vec![0xff, 0xfe]).is_err());
    }

    #[test]
    fn oversized_download_rejected() {
        let bytes = vec![b'x'; MAX_CLIPBOARD_BYTES + 1];
        assert!(decode_text(bytes).is_err());
    }

    #[test]
    fn empty_download_rejected() {
        assert!(decode_text(Vec::new()).is_err());
    }
}
