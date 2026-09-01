// Hide the console window in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Drop into CLI mode if the first arg is a known subcommand, else run the GUI.
    if let Some(first) = args.first() {
        if kansolo_lib::cli::SUBCOMMANDS.contains(&first.as_str()) {
            kansolo_lib::cli::dispatch_and_exit(&args);
        }
    }
    kansolo_lib::run()
}
