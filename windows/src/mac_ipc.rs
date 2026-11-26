use serde::Deserialize;
use ssh2::{ExtendedData, Session};
use std::error::Error;
use std::io::Read;

const PROTOCOL_VERSION: u32 = 1;
const MAX_RESPONSE_BYTES: usize = 4096;

const STATUS_COMMAND: &str = r#"/usr/bin/nc -U -w 3 "$HOME/.reimsbridge/status.sock" </dev/null"#;

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

pub fn get_status(session: &Session) -> Result<MacStatus, Box<dyn Error>> {
    let mut channel = session.channel_session()?;

    channel.handle_extended_data(ExtendedData::Ignore)?;
    channel.exec(STATUS_COMMAND)?;
    channel.send_eof()?;

    let mut response = Vec::new();

    (&mut channel)
        .take((MAX_RESPONSE_BYTES + 1) as u64)
        .read_to_end(&mut response)?;

    if response.len() > MAX_RESPONSE_BYTES {
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

    parse_status(&response)
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
        "protocol": 1,
        "status": "ok",
        "changeCount": 14,
        "hasPlainText": true,
        "isExcluded": false
    }"#;

    #[test]
    fn valid_response() {
        let status = parse_status(VALID_RESPONSE).unwrap();

        assert_eq!(status.protocol, 1);
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
            "protocol": 1,
            "status": "error",
            "changeCount": 14,
            "hasPlainText": true,
            "isExcluded": false
        }"#;

        assert!(parse_status(response).is_err());
    }

    #[test]
    fn oversized_response_rejected() {
        let response = vec![b'x'; MAX_RESPONSE_BYTES + 1];

        assert!(parse_status(&response).is_err());
    }
}
