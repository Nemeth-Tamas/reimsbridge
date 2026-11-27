use crate::ssh::{HOST, PORT, Password, wipe};
use std::error::Error;
use std::ffi::c_void;
use std::io;
use std::mem::size_of;
use std::ptr::{NonNull, null, null_mut};
use std::slice;
use windows_sys::Win32::Foundation::{ERROR_CANCELLED, ERROR_NOT_FOUND, HWND};
use windows_sys::Win32::Security::Credentials::{
    CRED_MAX_CREDENTIAL_BLOB_SIZE, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW,
    CREDUI_INFOW, CREDUI_MAX_USERNAME_LENGTH, CREDUIWIN_CHECKBOX, CREDUIWIN_GENERIC, CredDeleteW,
    CredFree, CredReadW, CredUIPromptForWindowsCredentialsW, CredUnPackAuthenticationBufferW,
    CredWriteW,
};
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::core::w;

pub struct Credentials {
    pub username: String,
    pub password: Password,
    pub remember: bool,
}

#[derive(Default)]
struct AuthBuffer {
    data: *mut c_void,
    length: u32,
}

impl Drop for AuthBuffer {
    fn drop(&mut self) {
        if !self.data.is_null() {
            unsafe {
                wipe(slice::from_raw_parts_mut(
                    self.data.cast::<u8>(),
                    self.length as usize,
                ));
                CoTaskMemFree(self.data);
            }
        }
    }
}

struct WidePassword([u16; 1024]);

impl Drop for WidePassword {
    fn drop(&mut self) {
        wipe(&mut self.0);
    }
}

pub fn prompt(owner: HWND) -> Result<Option<Credentials>, Box<dyn Error>> {
    let info = CREDUI_INFOW {
        cbSize: size_of::<CREDUI_INFOW>() as u32,
        hwndParent: owner,
        pszMessageText: w!("Enter the macOS SSH account and password for ReimsBridge."),
        pszCaptionText: w!("ReimsBridge SSH"),
        ..Default::default()
    };
    let mut package = 0;
    let mut auth = AuthBuffer::default();
    let mut remember = 0;
    let result = unsafe {
        CredUIPromptForWindowsCredentialsW(
            &info,
            0,
            &mut package,
            null(),
            0,
            &mut auth.data,
            &mut auth.length,
            &mut remember,
            CREDUIWIN_GENERIC | CREDUIWIN_CHECKBOX,
        )
    };

    if result == ERROR_CANCELLED {
        return Ok(None);
    }
    if result != 0 {
        return Err(io::Error::from_raw_os_error(result as i32).into());
    }
    if auth.data.is_null() {
        return Err("Windows returned an empty credential buffer".into());
    }

    let mut username = [0u16; CREDUI_MAX_USERNAME_LENGTH as usize + 1];
    let mut domain = [0u16; CREDUI_MAX_USERNAME_LENGTH as usize + 1];
    let mut password = WidePassword([0u16; 1024]);
    let mut username_len = username.len() as u32;
    let mut domain_len = domain.len() as u32;
    let mut password_len = password.0.len() as u32;

    if unsafe {
        CredUnPackAuthenticationBufferW(
            0,
            auth.data,
            auth.length,
            username.as_mut_ptr(),
            &mut username_len,
            domain.as_mut_ptr(),
            &mut domain_len,
            password.0.as_mut_ptr(),
            &mut password_len,
        )
    } == 0
    {
        return Err(io::Error::last_os_error().into());
    }

    let username = decode(&username)?;
    if username.is_empty() {
        return Err("Enter a macOS SSH username".into());
    }

    Ok(Some(Credentials {
        username,
        password: Password::new(decode(&password.0)?),
        remember: remember != 0,
    }))
}

fn decode(buffer: &[u16]) -> Result<String, std::string::FromUtf16Error> {
    let len = buffer
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(buffer.len());
    String::from_utf16(&buffer[..len])
}

fn target_name() -> Vec<u16> {
    format!("ReimsBridge/ssh/{HOST}:{PORT}")
        .encode_utf16()
        .chain([0])
        .collect()
}

const BLOB_VERSION: u8 = 1;
const BLOB_HEADER_LEN: usize = 3;

struct CredentialBlob(Vec<u8>);

impl Drop for CredentialBlob {
    fn drop(&mut self) {
        wipe(&mut self.0);
    }
}

fn encode_saved(credentials: &Credentials) -> Result<CredentialBlob, Box<dyn Error>> {
    let username = credentials.username.as_bytes();
    let password = credentials.password.as_bytes();
    let size = BLOB_HEADER_LEN + username.len() + password.len();
    if size > CRED_MAX_CREDENTIAL_BLOB_SIZE as usize {
        return Err("SSH credentials exceed the Windows Credential Manager size limit".into());
    }

    // Generic credentials carry both fields in the app-defined blob.
    let mut blob = Vec::with_capacity(size);
    blob.push(BLOB_VERSION);
    blob.extend_from_slice(&(username.len() as u16).to_le_bytes());
    blob.extend_from_slice(username);
    blob.extend_from_slice(password);
    Ok(CredentialBlob(blob))
}

fn decode_saved(blob: &[u8]) -> Result<Credentials, Box<dyn Error>> {
    let (header, payload) = blob
        .split_at_checked(BLOB_HEADER_LEN)
        .ok_or("Saved SSH credential record is incomplete")?;
    if header[0] != BLOB_VERSION {
        return Err("Saved SSH credential record has an unsupported version".into());
    }

    let username_len = usize::from(u16::from_le_bytes([header[1], header[2]]));
    let (username, password) = payload
        .split_at_checked(username_len)
        .ok_or("Saved SSH credential record is incomplete")?;
    if username.is_empty() || username.contains(&0) {
        return Err("Saved SSH credential has an invalid username".into());
    }

    Ok(Credentials {
        username: std::str::from_utf8(username)?.to_owned(),
        password: Password::new(std::str::from_utf8(password)?.to_owned()),
        remember: false,
    })
}

struct NativeCredential(NonNull<CREDENTIALW>);

impl Drop for NativeCredential {
    fn drop(&mut self) {
        unsafe {
            let stored = self.0.as_ref();
            if !stored.CredentialBlob.is_null() && stored.CredentialBlobSize != 0 {
                wipe(slice::from_raw_parts_mut(
                    stored.CredentialBlob,
                    stored.CredentialBlobSize as usize,
                ));
            }
            CredFree(self.0.as_ptr().cast());
        }
    }
}

pub fn load_saved() -> Result<Option<Credentials>, Box<dyn Error>> {
    let target = target_name();
    let mut raw = null_mut();
    if unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut raw) } == 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(ERROR_NOT_FOUND as i32) {
            return Ok(None);
        }
        return Err(error.into());
    }

    let record = NativeCredential(NonNull::new(raw).ok_or("Empty saved credential record")?);
    let stored = unsafe { record.0.as_ref() };
    let length = stored.CredentialBlobSize;
    if !(BLOB_HEADER_LEN as u32..=CRED_MAX_CREDENTIAL_BLOB_SIZE).contains(&length)
        || stored.CredentialBlob.is_null()
    {
        return Err("Saved SSH credential has an invalid record".into());
    }
    let blob = unsafe { slice::from_raw_parts(stored.CredentialBlob, length as usize) };
    decode_saved(blob).map(Some)
}

pub fn save(credentials: &Credentials) -> Result<(), Box<dyn Error>> {
    let mut target = target_name();
    let mut username: Vec<u16> = credentials.username.encode_utf16().chain([0]).collect();
    let mut blob = encode_saved(credentials)?;

    let record = CREDENTIALW {
        Type: CRED_TYPE_GENERIC,
        TargetName: target.as_mut_ptr(),
        CredentialBlobSize: blob.0.len() as u32,
        CredentialBlob: blob.0.as_mut_ptr(),
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        UserName: username.as_mut_ptr(),
        ..Default::default()
    };

    if unsafe { CredWriteW(&record, 0) } == 0 {
        return Err(io::Error::last_os_error().into());
    }

    Ok(())
}

pub fn forget() -> io::Result<()> {
    let target = target_name();
    if unsafe { CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) } == 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(ERROR_NOT_FOUND as i32) {
            return Err(error);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_blob_preserves_both_fields_and_rejects_truncation() {
        let credentials = Credentials {
            username: "test-user".into(),
            password: Password::new("synthetic test: á 🦀".into()),
            remember: true,
        };
        let blob = encode_saved(&credentials).unwrap();
        let loaded = decode_saved(&blob.0).unwrap();

        assert_eq!(loaded.username, credentials.username);
        assert_eq!(loaded.password.as_bytes(), credentials.password.as_bytes());
        assert!(!loaded.remember);
        assert!(decode_saved(&blob.0[..BLOB_HEADER_LEN]).is_err());
        assert!(decode_saved(&[BLOB_VERSION + 1, 1, 0, b'u']).is_err());
    }
}
