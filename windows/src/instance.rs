use std::error::Error;
use std::io;
use std::ptr::null;
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, SetLastError,
};
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::core::w;

pub struct InstanceGuard(HANDLE);

impl InstanceGuard {
    pub fn tray() -> Result<Self, Box<dyn Error>> {
        Self::claim(
            w!("Local\\ReimsBridge.Tray"),
            "ReimsBridge is already in the tray",
        )
    }

    pub fn watcher() -> Result<Self, Box<dyn Error>> {
        Self::claim(
            w!("Local\\ReimsBridge.Watcher"),
            "A ReimsBridge watcher or acceptance test is already running",
        )
    }

    fn claim(name: *const u16, message: &'static str) -> Result<Self, Box<dyn Error>> {
        // The named object lives as long as its first instance holds the handle.
        let (handle, status) = unsafe {
            SetLastError(0);
            let handle = CreateMutexW(null(), 0, name);
            (handle, GetLastError())
        };

        if handle.is_null() {
            return Err(io::Error::from_raw_os_error(status as i32).into());
        }

        let guard = Self(handle);
        if status == ERROR_ALREADY_EXISTS {
            return Err(message.into());
        }

        Ok(guard)
    }
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_guard_rejects_duplicates_and_allows_reacquisition() {
        let name: Vec<u16> = format!("Local\\ReimsBridge.Test.{}", std::process::id())
            .encode_utf16()
            .chain([0])
            .collect();
        let claim = || InstanceGuard::claim(name.as_ptr(), "already running");

        let first = claim().expect("initial claim");
        assert!(claim().is_err());
        drop(first);
        assert!(claim().is_ok());
    }
}
