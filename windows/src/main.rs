fn main() {
    if let Err(error) = reimsbridge_windows::run_cli() {
        eprintln!("[ERROR] {error}");
        std::process::exit(1);
    }
}
