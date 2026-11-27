use std::error::Error;

use crate::validation::{MAX_CLIPBOARD_BYTES, validate_text};

pub const TOKEN: &str = "clipboard-bundle-v1";
pub const MAX_PNG_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_IMAGE_PIXELS: u64 = 16_777_216;
const HEADER_BYTES: usize = 16;
pub const MAX_BUNDLE_BYTES: usize = HEADER_BYTES + 3 * MAX_CLIPBOARD_BYTES + MAX_PNG_BYTES;
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ClipboardBundle {
    pub text: String,
    pub rtf: Vec<u8>,
    pub html: String,
    pub png: Vec<u8>,
}

impl ClipboardBundle {
    pub fn has_extended_content(&self) -> bool {
        !self.rtf.is_empty() || !self.html.is_empty() || !self.png.is_empty()
    }

    pub fn byte_len(&self) -> usize {
        self.text.len() + self.rtf.len() + self.html.len() + self.png.len()
    }

    fn validate(&self) -> Result<(), Box<dyn Error>> {
        if !self.has_extended_content() {
            return Err("Clipboard bundle contains no extended format".into());
        }
        if !self.text.is_empty() {
            validate_text(&self.text)?;
        }
        if !self.html.is_empty() {
            validate_text(&self.html)?;
        }
        if !self.rtf.is_empty() && !valid_rtf(&self.rtf) {
            return Err("Invalid or oversized RTF payload".into());
        }
        if !self.png.is_empty() && canonical_png(&self.png)? != self.png {
            return Err("PNG payload has trailing bytes".into());
        }
        Ok(())
    }

    pub fn encode(&self) -> Result<Vec<u8>, Box<dyn Error>> {
        self.validate()?;
        let parts = [
            self.text.as_bytes(),
            &self.rtf,
            self.html.as_bytes(),
            &self.png,
        ];
        let mut payload = Vec::with_capacity(HEADER_BYTES + self.byte_len());
        for part in parts {
            payload.extend_from_slice(&(part.len() as u32).to_be_bytes());
        }
        for part in parts {
            payload.extend_from_slice(part);
        }
        Ok(payload)
    }

    pub fn decode(payload: &[u8]) -> Result<Self, Box<dyn Error>> {
        let (header, mut body) = payload
            .split_at_checked(HEADER_BYTES)
            .ok_or("Clipboard bundle header is missing")?;
        let mut take = |offset, limit| -> Result<&[u8], Box<dyn Error>> {
            let length = u32::from_be_bytes(header[offset..offset + 4].try_into()?) as usize;
            if length > limit {
                return Err("Clipboard bundle member exceeds size limit".into());
            }
            let (part, rest) = body
                .split_at_checked(length)
                .ok_or("Clipboard bundle ended inside a member")?;
            body = rest;
            Ok(part)
        };

        let bundle = Self {
            text: std::str::from_utf8(take(0, MAX_CLIPBOARD_BYTES)?)?.to_owned(),
            rtf: take(4, MAX_CLIPBOARD_BYTES)?.to_vec(),
            html: std::str::from_utf8(take(8, MAX_CLIPBOARD_BYTES)?)?.to_owned(),
            png: take(12, MAX_PNG_BYTES)?.to_vec(),
        };
        if !body.is_empty() {
            return Err("Clipboard bundle contains trailing bytes".into());
        }
        bundle.validate()?;
        Ok(bundle)
    }
}

pub fn valid_rtf(bytes: &[u8]) -> bool {
    bytes.len() <= MAX_CLIPBOARD_BYTES
        && bytes.starts_with(b"{\\rtf")
        && bytes
            .iter()
            .rev()
            .find(|&&byte| !byte.is_ascii_whitespace())
            == Some(&b'}')
}

pub fn valid_image_dimensions(width: u32, height: u32) -> bool {
    width != 0 && height != 0 && u64::from(width) * u64::from(height) <= MAX_IMAGE_PIXELS
}

pub fn canonical_png(bytes: &[u8]) -> Result<&[u8], Box<dyn Error>> {
    if bytes.len() > MAX_PNG_BYTES
        || !bytes.starts_with(PNG_SIGNATURE)
        || bytes.len() < 33
        || &bytes[8..16] != b"\x00\x00\x00\x0dIHDR"
    {
        return Err("Invalid or oversized PNG payload".into());
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into()?);
    if !valid_image_dimensions(width, height) {
        return Err("PNG dimensions exceed the clipboard limit".into());
    }

    let mut offset = PNG_SIGNATURE.len();
    while let Some(header) = bytes.get(offset..offset + 8) {
        let length = u32::from_be_bytes(header[..4].try_into()?) as usize;
        let end = offset
            .checked_add(length)
            .and_then(|n| n.checked_add(12))
            .filter(|&n| n <= bytes.len())
            .ok_or("Truncated PNG chunk")?;
        if &header[4..] == b"IEND" && length == 0 {
            return Ok(&bytes[..end]);
        }
        offset = end;
    }
    Err("PNG end chunk is missing".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_preserves_rich_representations_and_rejects_truncated_frames() {
        let bundle = ClipboardBundle {
            text: "Árvizű 🚀".into(),
            rtf: b"{\\rtf1\\ansi \\b Test\\b0}".to_vec(),
            html: "<strong>Árvizű 🚀</strong>".into(),
            ..ClipboardBundle::default()
        };
        let mut encoded = bundle.encode().unwrap();
        assert_eq!(ClipboardBundle::decode(&encoded).unwrap(), bundle);
        assert!(ClipboardBundle::decode(&encoded[..encoded.len() - 1]).is_err());
        encoded.push(0);
        assert!(ClipboardBundle::decode(&encoded).is_err());
        encoded[0..4].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(ClipboardBundle::decode(&encoded).is_err());
    }
}
