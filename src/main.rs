#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod licensing;
mod model;
mod platform;
mod profiles;
mod server;
#[cfg(windows)]
mod startup;
mod store;
mod worker;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--repair-firewall") {
        let result = platform::repair_network_entry(&args[2..]);
        if let Err(e) = &result {
            platform::show_error(e);
        }
        std::process::exit(if result.is_ok() { 0 } else { 1 });
    }
    if args.get(1).map(String::as_str) == Some("--worker") {
        let code = worker::entry(&args[2..]);
        platform::finish_worker(code);
    }
    if let Err(e) = server::run(&args[1..]) {
        eprintln!("{e}");
        platform::show_error(&e);
        std::process::exit(1);
    }
}
