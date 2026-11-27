use crate::instance::InstanceGuard;
use crate::watch_worker::{Update, Worker};
use crate::{credentials, login_startup};
use std::error::Error;
use std::io;
use std::mem::size_of;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETFOCUS,
    NIM_SETVERSION, NIN_SELECT, NOTIFY_ICON_MESSAGE, NOTIFYICON_VERSION_4, NOTIFYICONDATAW,
    Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, GetCursorPos, GetMessageW, IDI_APPLICATION, IDI_INFORMATION, IDI_WARNING,
    KillTimer, LoadIconW, MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MF_GRAYED, MF_SEPARATOR,
    MF_STRING, MSG, MessageBoxW, PostMessageW, RegisterClassW, RegisterWindowMessageW,
    SetForegroundWindow, SetTimer, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu,
    TranslateMessage, WM_APP, WM_CLOSE, WM_CONTEXTMENU, WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP,
    WNDCLASSW, WS_POPUP,
};
use windows_sys::core::{PCWSTR, w};

const TRAY_ICON_MESSAGE: u32 = WM_APP + 1;
const OPEN_MENU_MESSAGE: u32 = WM_APP + 2;
const KEY_SELECT: u32 = NIN_SELECT | 1;
const TIMER_ID: usize = 1;
const TIMER_INTERVAL_MS: u32 = 250;

const START: usize = 1;
const STOP: usize = 2;
const VIEW_ERROR: usize = 3;
const QUIT: usize = 4;
const START_DIFFERENT: usize = 5;
const FORGET_SAVED: usize = 6;
const LOGIN_STARTUP: usize = 7;

static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);
static RESTORE_ICON: AtomicBool = AtomicBool::new(false);
static ICON_VERSION_4: AtomicBool = AtomicBool::new(false);
static CLOSE_REQUESTED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Status {
    #[default]
    Stopped,
    Connecting,
    Reconnecting,
    Syncing,
    Stopping,
    Error,
}

impl Status {
    fn label(self) -> &'static str {
        match self {
            Self::Stopped => "Stopped",
            Self::Connecting => "Connecting",
            Self::Reconnecting => "Reconnecting",
            Self::Syncing => "Synchronizing",
            Self::Stopping => "Stopping",
            Self::Error => "Connection error",
        }
    }

    fn icon(self) -> PCWSTR {
        match self {
            Self::Syncing => IDI_INFORMATION,
            Self::Error | Self::Reconnecting => IDI_WARNING,
            _ => IDI_APPLICATION,
        }
    }
}

#[derive(Default)]
struct TrayState {
    worker: Option<Worker>,
    phase: Status,
    retry_details: Option<String>,
    credential_warning: Option<String>,
    file_notice: Option<String>,
    error: Option<String>,
    quitting: bool,
}

impl TrayState {
    fn status(&self) -> Status {
        match &self.worker {
            Some(worker) if worker.stopping() => Status::Stopping,
            Some(_) => self.phase,
            None if self.error.is_some() => Status::Error,
            None => Status::Stopped,
        }
    }

    fn start(&mut self, owner: HWND, use_saved: bool) {
        if self.worker.is_some() || self.quitting {
            return;
        }

        let mut credential_warning = None;
        let result = if use_saved {
            match credentials::load_saved() {
                Ok(Some(credentials)) => Ok(Some(credentials)),
                Ok(None) => credentials::prompt(owner),
                Err(error) => {
                    credential_warning = Some(format!(
                        "Could not read saved SSH credentials from Windows Credential Manager: {error}"
                    ));
                    credentials::prompt(owner)
                }
            }
        } else {
            credentials::prompt(owner)
        };

        match result {
            Ok(Some(credentials)) => self.launch_worker(credentials, credential_warning),
            Ok(None) => {}
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    fn launch_worker(&mut self, credentials: credentials::Credentials, warning: Option<String>) {
        match Worker::start(credentials) {
            Ok(worker) => {
                self.phase = Status::Connecting;
                self.error = None;
                self.retry_details = None;
                self.credential_warning = warning;
                self.file_notice = None;
                self.worker = Some(worker);
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    fn start_at_login(&mut self) {
        match credentials::load_saved() {
            Ok(Some(credentials)) => self.launch_worker(credentials, None),
            Ok(None) => {
                self.error = Some("Save an SSH login before using automatic synchronization".into())
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    fn toggle_login_startup(&mut self) {
        let result = (|| -> Result<(), Box<dyn Error>> {
            let enable = !login_startup::enabled()?;
            if enable && credentials::load_saved()?.is_none() {
                return Err("Save your SSH login before enabling Start at Login".into());
            }
            login_startup::set_enabled(enable)?;
            Ok(())
        })();
        self.error = result.err().map(|error| error.to_string());
    }

    fn forget_saved(&mut self) {
        if self.worker.is_none() {
            self.error = credentials::forget().err().map(|error| error.to_string());
            self.credential_warning = None;
        }
    }

    fn details(&self) -> Option<&str> {
        self.error
            .as_deref()
            .or(self.retry_details.as_deref())
            .or(self.credential_warning.as_deref())
            .or(self.file_notice.as_deref())
    }

    fn stop(&self) {
        if let Some(worker) = &self.worker {
            worker.request_stop();
        }
    }

    fn quit(&mut self) {
        self.quitting = true;
        self.stop();
    }

    fn update_worker(&mut self) {
        if let Some(worker) = &self.worker {
            for update in worker.updates.try_iter() {
                match update {
                    Update::Active => {
                        self.phase = Status::Syncing;
                        self.retry_details = None;
                    }
                    Update::Reconnecting { delay, reason } => {
                        self.phase = Status::Reconnecting;
                        self.retry_details = Some(format!(
                            "{reason}\n\nRetry delay: {} seconds",
                            delay.as_secs()
                        ));
                    }
                    Update::CredentialWarning(warning) => self.credential_warning = Some(warning),
                    Update::FileNotice(message) => self.file_notice = Some(message),
                }
            }
        }

        let Some(worker) = self.worker.take_if(|worker| worker.thread.is_finished()) else {
            return;
        };

        let stopped = worker.stopping();
        self.retry_details = None;
        self.error = match worker.thread.join() {
            Ok(_) if stopped => None,
            Ok(Ok(())) => None,
            Ok(Err(error)) => Some(error),
            Err(_) => Some("Clipboard synchronization worker terminated unexpectedly".into()),
        };
    }
}

impl Drop for TrayState {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            worker.request_stop();
            let _ = worker.thread.join();
        }
    }
}

struct TrayWindow(HWND);

impl TrayWindow {
    fn new() -> io::Result<Self> {
        let taskbar_created = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
        if taskbar_created == 0 {
            return Err(io::Error::last_os_error());
        }
        TASKBAR_CREATED.store(taskbar_created, Ordering::Relaxed);

        let instance = unsafe { GetModuleHandleW(null()) };
        if instance.is_null() {
            return Err(io::Error::last_os_error());
        }

        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: w!("ReimsBridge.TrayWindow"),
            ..Default::default()
        };
        if unsafe { RegisterClassW(&class) } == 0 {
            return Err(io::Error::last_os_error());
        }

        // A hidden top-level window receives Explorer's taskbar broadcasts.
        let window = unsafe {
            CreateWindowExW(
                0,
                class.lpszClassName,
                w!("ReimsBridge"),
                WS_POPUP,
                0,
                0,
                0,
                0,
                null_mut(),
                null_mut(),
                instance,
                null(),
            )
        };
        if window.is_null() {
            return Err(io::Error::last_os_error());
        }
        let window = Self(window);
        if unsafe { SetTimer(window.0, TIMER_ID, TIMER_INTERVAL_MS, None) } == 0 {
            return Err(io::Error::last_os_error());
        }

        Ok(window)
    }

    fn notify_icon(&self, action: NOTIFY_ICON_MESSAGE, status: Status) -> bool {
        let tip = format!("ReimsBridge - {}", status.label());
        let mut data = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.0,
            uID: 1,
            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP,
            uCallbackMessage: TRAY_ICON_MESSAGE,
            hIcon: unsafe { LoadIconW(null_mut(), status.icon()) },
            ..Default::default()
        };
        for (out, unit) in data.szTip.iter_mut().take(127).zip(tip.encode_utf16()) {
            *out = unit;
        }

        let succeeded = unsafe { Shell_NotifyIconW(action, &data) } != 0;
        if action == NIM_ADD && succeeded {
            data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            let version_set = unsafe { Shell_NotifyIconW(NIM_SETVERSION, &data) } != 0;
            ICON_VERSION_4.store(version_set, Ordering::Relaxed);
        }

        succeeded
    }

    fn show_menu(&self, state: &TrayState, location: WPARAM, event: u32) -> usize {
        let menu = unsafe { CreatePopupMenu() };
        if menu.is_null() {
            return 0;
        }

        let status: Vec<u16> = state.status().label().encode_utf16().chain([0]).collect();
        let disabled_if = |disabled| if disabled { MF_GRAYED } else { 0 };
        let mut point = POINT {
            x: location as i16 as i32,
            y: (location >> 16) as i16 as i32,
        };

        unsafe {
            if matches!(event, WM_RBUTTONUP | WM_LBUTTONUP) {
                GetCursorPos(&mut point);
            }

            AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, status.as_ptr());
            AppendMenuW(menu, MF_SEPARATOR, 0, null());
            AppendMenuW(
                menu,
                MF_STRING | disabled_if(state.worker.is_some() || state.quitting),
                START,
                w!("Start Synchronization"),
            );
            AppendMenuW(
                menu,
                MF_STRING | disabled_if(state.worker.is_some() || state.quitting),
                START_DIFFERENT,
                w!("Start with Different Credentials..."),
            );
            AppendMenuW(
                menu,
                MF_STRING
                    | disabled_if(state.worker.is_none() || state.status() == Status::Stopping),
                STOP,
                w!("Stop Synchronization"),
            );
            AppendMenuW(
                menu,
                MF_STRING | disabled_if(state.details().is_none()),
                VIEW_ERROR,
                w!("View Connection Details"),
            );
            AppendMenuW(
                menu,
                MF_STRING | disabled_if(state.worker.is_some() || state.quitting),
                FORGET_SAVED,
                w!("Forget Saved Credentials"),
            );
            AppendMenuW(
                menu,
                MF_STRING,
                LOGIN_STARTUP,
                if login_startup::enabled().unwrap_or(false) {
                    w!("Disable Start at Login")
                } else {
                    w!("Enable Start at Login")
                },
            );
            AppendMenuW(menu, MF_SEPARATOR, 0, null());
            AppendMenuW(menu, MF_STRING, QUIT, w!("Quit ReimsBridge"));

            SetForegroundWindow(self.0);
            let selection = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
                point.x,
                point.y,
                0,
                self.0,
                null(),
            );
            PostMessageW(self.0, WM_NULL, 0, 0);
            DestroyMenu(menu);
            selection as usize
        }
    }
}

impl Drop for TrayWindow {
    fn drop(&mut self) {
        self.notify_icon(NIM_DELETE, Status::Stopped);
        unsafe {
            KillTimer(self.0, TIMER_ID);
            DestroyWindow(self.0);
        }
    }
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        TRAY_ICON_MESSAGE => {
            let event = lparam as u32 & 0xffff;
            let show_menu = if ICON_VERSION_4.load(Ordering::Relaxed) {
                matches!(event, WM_CONTEXTMENU | NIN_SELECT | KEY_SELECT)
            } else {
                matches!(event, WM_RBUTTONUP | WM_LBUTTONUP)
            };
            if show_menu {
                unsafe { PostMessageW(window, OPEN_MENU_MESSAGE, wparam, event as LPARAM) };
            }
            0
        }
        WM_CLOSE => {
            CLOSE_REQUESTED.store(true, Ordering::Relaxed);
            0
        }
        message if message == TASKBAR_CREATED.load(Ordering::Relaxed) => {
            RESTORE_ICON.store(true, Ordering::Relaxed);
            0
        }
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

pub fn run() -> Result<(), Box<dyn Error>> {
    let _instance = InstanceGuard::tray()?;
    let window = TrayWindow::new()?;
    let mut state = TrayState::default();
    let mut login_pending = std::env::args_os().skip(1).any(|arg| arg == "--login");
    let mut displayed = state.status();
    if window.notify_icon(NIM_ADD, displayed) {
        if login_pending {
            state.start_at_login();
            login_pending = false;
        }
    } else if login_pending {
        RESTORE_ICON.store(true, Ordering::Relaxed);
    } else {
        return Err("Could not add ReimsBridge to the Windows notification area".into());
    }

    let mut message = MSG::default();
    loop {
        match unsafe { GetMessageW(&mut message, null_mut(), 0, 0) } {
            -1 => return Err(io::Error::last_os_error().into()),
            0 => return Ok(()),
            _ => {}
        }

        state.update_worker();

        match message.message {
            OPEN_MENU_MESSAGE if message.hwnd == window.0 => {
                let action = window.show_menu(&state, message.wParam, message.lParam as u32);
                state.update_worker();
                match action {
                    START => state.start(window.0, true),
                    START_DIFFERENT => state.start(window.0, false),
                    FORGET_SAVED => state.forget_saved(),
                    LOGIN_STARTUP => state.toggle_login_startup(),
                    STOP => state.stop(),
                    VIEW_ERROR => {
                        if let Some(details) = state.details() {
                            let icon = if state.error.is_some() {
                                MB_ICONERROR
                            } else {
                                MB_ICONINFORMATION
                            };
                            show_message(window.0, details, icon);
                        }
                    }
                    QUIT => state.quit(),
                    _ => {}
                }
                window.notify_icon(NIM_SETFOCUS, state.status());
            }
            _ => unsafe {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            },
        }

        if CLOSE_REQUESTED.swap(false, Ordering::Relaxed) {
            state.quit();
        }

        if state.quitting && state.worker.is_none() {
            return Ok(());
        }

        let status = state.status();
        if RESTORE_ICON.swap(false, Ordering::Relaxed) {
            if !window.notify_icon(NIM_ADD, status) && !window.notify_icon(NIM_MODIFY, status) {
                RESTORE_ICON.store(true, Ordering::Relaxed);
            } else if login_pending {
                state.start_at_login();
                login_pending = false;
            }
        } else if displayed != status && !window.notify_icon(NIM_MODIFY, status) {
            RESTORE_ICON.store(true, Ordering::Relaxed);
        }
        displayed = status;
    }
}

fn show_message(owner: HWND, text: &str, icon: u32) {
    let message: Vec<u16> = text.encode_utf16().chain([0]).collect();
    unsafe { MessageBoxW(owner, message.as_ptr(), w!("ReimsBridge"), MB_OK | icon) };
}

pub fn show_launch_error(error: &str) {
    show_message(null_mut(), error, MB_ICONERROR);
}
