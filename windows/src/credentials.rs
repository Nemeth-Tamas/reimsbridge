use crate::ssh::{Password, wipe};
use std::error::Error;
use std::ffi::c_void;
use std::io;
use std::mem::size_of;
use std::ptr::null;
use std::slice;
use windows_sys::Win32::Foundation::{ERROR_CANCELLED, HWND};
use windows_sys::Win32::Security::Credentials::{
    CREDUI_INFOW, CREDUI_MAX_USERNAME_LENGTH, CREDUIWIN_GENERIC,
    CredUIPromptForWindowsCredentialsW, CredUnPackAuthenticationBufferW,
};
use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::core::w;

pub struct Credentials {
    pub username: String,
    pub password: Password,
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
    let result = unsafe {
        CredUIPromptForWindowsCredentialsW(
            &info,
            0,
            &mut package,
            null(),
            0,
            &mut auth.data,
            &mut auth.length,
            std::ptr::null_mut(),
            CREDUIWIN_GENERIC,
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
    }))
}

fn decode(buffer: &[u16]) -> Result<String, std::string::FromUtf16Error> {
    let len = buffer
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(buffer.len());
    String::from_utf16(&buffer[..len])
}
