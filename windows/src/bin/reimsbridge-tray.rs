#![windows_subsystem = "windows"]

fn main() {
    if let Err(error) = reimsbridge_windows::run_tray() {
        reimsbridge_windows::show_launch_error(&error.to_string());
        std::process::exit(1);
    }
}
