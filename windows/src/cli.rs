use crate::{clipboard, instance::InstanceGuard, mac_ipc, ssh, sync, watch};
use ssh2::Session;
use std::env;
use std::error::Error;
use std::io::Read;

fn transport_test(session: &Session) -> Result<(), Box<dyn Error>> {
    let mut channel = session.channel_session()?;

    channel.exec("/usr/bin/whoami")?;

    let mut output = String::new();

    channel.read_to_string(&mut output)?;
    channel.wait_close()?;

    if channel.exit_status()? != 0 {
        return Err("Remote command failed".into());
    }

    println!("[SSH] Remote user: {}", output.trim());
    println!("[SSH] Transport test passed");

    Ok(())
}

fn print_help() {
    println!("Usage:");
    println!("  reimsbridge-windows clipboard-check");
    println!("  reimsbridge-windows clipboard-metadata");
    println!("  reimsbridge-windows clipboard-test");
    println!("  reimsbridge-windows ssh-test <username>");
    println!("  reimsbridge-windows mac-status <username>");
    println!("  reimsbridge-windows mac-receive <username>");
    println!("  reimsbridge-windows mac-send <username>");
    println!("  reimsbridge-windows mac-conditional-test <username>");
    println!("  reimsbridge-windows watch <username>");
    println!("  reimsbridge-windows watch-test <username>");
    println!("  reimsbridge-windows send <username>");
    println!("  reimsbridge-windows receive <username>");
    println!("  reimsbridge-windows roundtrip-test <username>");
    println!();
    println!("Launch reimsbridge-tray.exe for the Windows notification area.");
}

pub fn run() -> Result<(), Box<dyn Error>> {
    println!("ReimsBridge v{}", env!("CARGO_PKG_VERSION"));
    println!("M2.3D - Automatic synchronization acceptance");
    println!();

    let mut args = env::args().skip(1);

    let Some(command) = args.next() else {
        print_help();
        return Ok(());
    };

    match command.as_str() {
        "help" | "--help" | "-h" => {
            print_help();
            return Ok(());
        }
        "clipboard-check" => return clipboard::check(),
        "clipboard-metadata" => return clipboard::check_metadata(),
        "clipboard-test" => return clipboard::self_test(),
        "ssh-test"
        | "send"
        | "receive"
        | "roundtrip-test"
        | "mac-status"
        | "mac-receive"
        | "mac-send"
        | "mac-conditional-test"
        | "watch"
        | "watch-test" => {}
        _ => {
            print_help();
            return Err("Unknown command".into());
        }
    }

    let username = args.next().ok_or("Missing SSH username")?;

    if args.next().is_some() {
        return Err("Unexpected command-line argument".into());
    }

    let _watcher = if matches!(command.as_str(), "watch" | "watch-test") {
        Some(InstanceGuard::watcher()?)
    } else {
        None
    };

    let session = ssh::connect_verified()?;
    let password = rpassword::prompt_password(format!("{username}@{}'s password: ", ssh::HOST))?;
    ssh::authenticate(&session, &username, &ssh::Password::new(password))?;

    match command.as_str() {
        "ssh-test" => transport_test(&session)?,
        "mac-status" => mac_ipc::check(&session)?,
        "mac-receive" => mac_ipc::receive(&session)?,
        "mac-send" => mac_ipc::send(&session)?,
        "mac-conditional-test" => mac_ipc::test_checked_write(&session)?,
        "watch" => watch::run(&session)?,
        "watch-test" => watch::test(&session)?,
        "send" => sync::send(&session)?,
        "receive" => sync::receive(&session)?,
        "roundtrip-test" => sync::roundtrip_test(&session)?,
        _ => unreachable!(),
    }

    session.disconnect(None, "ReimsBridge session complete", None)?;

    Ok(())
}
