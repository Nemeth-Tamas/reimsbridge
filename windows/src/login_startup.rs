use std::io;
use std::os::windows::ffi::OsStrExt;
use std::ptr::null_mut;
use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};
use windows_sys::core::{PCWSTR, w};

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const VALUE: PCWSTR = w!("ReimsBridge");

pub fn enabled() -> io::Result<bool> {
    let mut size = 0;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            VALUE,
            RRF_RT_REG_SZ,
            null_mut(),
            null_mut(),
            &mut size,
        )
    };
    match status {
        ERROR_SUCCESS => Ok(true),
        ERROR_FILE_NOT_FOUND => Ok(false),
        _ => Err(io::Error::from_raw_os_error(status as i32)),
    }
}

pub fn set_enabled(enabled: bool) -> io::Result<()> {
    let status = if enabled {
        let exe = std::env::current_exe()?;
        let command: Vec<u16> = [b'"' as u16]
            .into_iter()
            .chain(exe.as_os_str().encode_wide())
            .chain("\" --login".encode_utf16())
            .chain([0])
            .collect();
        if command.len() > 260 {
            return Err(io::Error::other(
                "ReimsBridge path is too long for Windows login startup",
            ));
        }
        unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                RUN_KEY,
                VALUE,
                REG_SZ,
                command.as_ptr().cast(),
                (command.len() * 2) as u32,
            )
        }
    } else {
        unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, VALUE) }
    };
    match status {
        ERROR_SUCCESS => Ok(()),
        ERROR_FILE_NOT_FOUND if !enabled => Ok(()),
        _ => Err(io::Error::from_raw_os_error(status as i32)),
    }
}
