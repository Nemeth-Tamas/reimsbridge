use serde::Deserialize;
use ssh2::{ExtendedData, Session};
use std::error::Error;
use std::io::{Read, Write};

use crate::{clipboard, validation};

const PROTOCOL_VERSION: u32 = 2;
const MAX_RESPONSE_BYTES: usize = 4096;
const MAX_HEADER_BYTES: usize = 64;

const IPC_COMMAND: &str = r#"/usr/bin/nc -U -w 3 "$HOME/.reimsbridge/status.sock""#;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MacStatus {
    pub protocol: u32,
    pub status: String,
    pub change_count: i64,
    pub has_plain_text: bool,
    pub is_excluded: bool,
}

fn parse_status(response: &[u8]) -> Result<MacStatus, Box<dyn Error>> {
    if response.len() > MAX_RESPONSE_BYTES {
        return Err("IPC response exceeds size limit".into());
    }

    let status: MacStatus =
        serde_json::from_slice(response).map_err(|_| "Invalid macOS IPC response")?;

    if status.protocol != PROTOCOL_VERSION {
        return Err("Unsupported macOS IPC protocol version".into());
    }

    if status.status != "ok" {
        return Err("macOS IPC returned an error".into());
    }

    Ok(status)
}

fn query(
    session: &Session,
    request: &[u8],
    max_response_bytes: usize,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut channel = session.channel_session()?;

    channel.handle_extended_data(ExtendedData::Ignore)?;
    channel.exec(IPC_COMMAND)?;

    channel.write_all(request)?;
    channel.flush()?;
    channel.send_eof()?;

    let mut response = Vec::new();

    (&mut channel)
        .take((max_response_bytes + 1) as u64)
        .read_to_end(&mut response)?;

    if response.len() > max_response_bytes {
        return Err("IPC response exceeds size limit".into());
    }

    channel.wait_close()?;

    if channel.exit_status()? != 0 {
        return Err("Cannot reach macOS IPC service. \
             Is ReimsBridge running with IPC enabled?"
            .into());
    }

    if channel.exit_signal()?.exit_signal.is_some() {
        return Err("macOS IPC command was terminated".into());
    }

    if response.is_empty() {
        return Err("macOS IPC returned no response".into());
    }

    Ok(response)
}

pub fn get_status(session: &Session) -> Result<MacStatus, Box<dyn Error>> {
    let response = query(session, b"status\n", MAX_RESPONSE_BYTES)?;

    parse_status(&response)
}

fn parse_read(mut response: Vec<u8>) -> Result<String, Box<dyn Error>> {
    let header_end = response
        .iter()
        .position(|&byte| byte == b'\n')
        .ok_or("Missing IPC response header")?;

    if header_end >= MAX_HEADER_BYTES {
        return Err("IPC response header exceeds size limit".into());
    }

    let header =
        std::str::from_utf8(&response[..header_end]).map_err(|_| "Invalid IPC response header")?;

    if header.starts_with("ERR ") {
        let message = match header {
            "ERR NO_TEXT" => "macOS clipboard contains no plain text",

            "ERR EXCLUDED" => "macOS clipboard is marked as excluded",

            "ERR OVERSIZED" => "macOS clipboard exceeds size limit",

            "ERR INVALID_TEXT" => "macOS clipboard contains invalid text",

            "ERR CLIPBOARD_CHANGED" => "macOS clipboard changed during read",

            _ => "Native macOS clipboard read failed",
        };

        return Err(message.into());
    }

    let declared_size = header
        .strip_prefix("OK ")
        .ok_or("Invalid IPC response status")?
        .parse::<usize>()
        .map_err(|_| "Invalid IPC payload length")?;

    if declared_size > validation::MAX_CLIPBOARD_BYTES {
        return Err("IPC payload exceeds size limit".into());
    }

    let actual_size = response.len() - header_end - 1;

    if actual_size != declared_size {
        return Err("IPC payload length mismatch".into());
    }

    let payload = response.split_off(header_end + 1);

    validation::decode_text(payload)
}

pub fn read_text(session: &Session) -> Result<String, Box<dyn Error>> {
    let response = query(
        session,
        b"read\n",
        validation::MAX_CLIPBOARD_BYTES + MAX_HEADER_BYTES,
    )?;

    parse_read(response)
}

pub fn receive(session: &Session) -> Result<(), Box<dyn Error>> {
    println!("[IPC] Native macOS -> Windows");

    let text = read_text(session)?;

    println!("[IPC] Received {} bytes", text.len());

    clipboard::write_text(&text)?;

    println!("[IPC] Native clipboard transfer: PASS");

    Ok(())
}

pub fn check(session: &Session) -> Result<(), Box<dyn Error>> {
    println!("[IPC] Querying native macOS application");

    let status = get_status(session)?;

    println!("[IPC] Protocol version: {}", status.protocol);
    println!("[IPC] Application status: {}", status.status);
    println!("[IPC] Clipboard change count: {}", status.change_count);
    println!("[IPC] Plain text available: {}", status.has_plain_text);
    println!("[IPC] Content excluded: {}", status.is_excluded);

    let eligible = status.has_plain_text && !status.is_excluded;

    println!("[IPC] Transfer candidate: {eligible}");
    println!("[IPC] Native macOS communication: PASS");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_RESPONSE: &[u8] = br#"{
        "protocol": 2,
        "status": "ok",
        "changeCount": 14,
        "hasPlainText": true,
        "isExcluded": false
    }"#;

    #[test]
    fn valid_response() {
        let status = parse_status(VALID_RESPONSE).unwrap();

        assert_eq!(status.protocol, 2);
        assert_eq!(status.change_count, 14);
        assert!(status.has_plain_text);
        assert!(!status.is_excluded);
    }

    #[test]
    fn malformed_json_rejected() {
        assert!(parse_status(b"not json").is_err());
    }

    #[test]
    fn missing_fields_rejected() {
        assert!(parse_status(br#"{"protocol":1}"#).is_err());
    }

    #[test]
    fn unsupported_protocol_rejected() {
        let response = br#"{
            "protocol": 99,
            "status": "ok",
            "changeCount": 14,
            "hasPlainText": true,
            "isExcluded": false
        }"#;

        assert!(parse_status(response).is_err());
    }

    #[test]
    fn remote_error_rejected() {
        let response = br#"{
            "protocol": 2,
            "status": "error",
            "changeCount": 14,
            "hasPlainText": true,
            "isExcluded": false
        }"#;

        let error = parse_status(response).unwrap_err();

        assert_eq!(error.to_string(), "macOS IPC returned an error");
    }

    #[test]
    fn oversized_response_rejected() {
        let response = vec![b'x'; MAX_RESPONSE_BYTES + 1];

        assert!(parse_status(&response).is_err());
    }

    #[test]
    fn valid_text_response() {
        let text = "Árvíztűrő 🚀\r\n    Code  \n";

        let mut response = format!("OK {}\n", text.len()).into_bytes();

        response.extend_from_slice(text.as_bytes());

        assert_eq!(parse_read(response).unwrap(), text);
    }

    #[test]
    fn truncated_payload_rejected() {
        assert!(parse_read(b"OK 10\nHello".to_vec()).is_err());
    }

    #[test]
    fn unexpected_extra_bytes_rejected() {
        assert!(parse_read(b"OK 3\nHello".to_vec()).is_err());
    }

    #[test]
    fn invalid_utf8_payload_rejected() {
        let mut response = b"OK 2\n".to_vec();

        response.extend_from_slice(&[0xff, 0xfe]);

        assert!(parse_read(response).is_err());
    }

    #[test]
    fn excluded_clipboard_rejected() {
        assert!(parse_read(b"ERR EXCLUDED\n".to_vec()).is_err());
    }

    #[test]
    fn oversized_payload_rejected() {
        let response = format!("OK {}\n", validation::MAX_CLIPBOARD_BYTES + 1);

        assert!(parse_read(response.into_bytes()).is_err());
    }
}
