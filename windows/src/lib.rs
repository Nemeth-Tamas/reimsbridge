mod cli;
mod clipboard;
mod credentials;
mod instance;
mod mac_ipc;
mod remote_clipboard;
mod ssh;
mod sync;
mod sync_engine;
mod sync_read;
mod tray;
mod validation;
mod watch;

pub use cli::run as run_cli;
pub use tray::{run as run_tray, show_launch_error};
