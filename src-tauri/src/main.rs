// Hide the console window in release builds on Windows. Keep this line.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // If the first arg is a known CLI subcommand, dispatch in CLI mode and
    // exit. Otherwise start the Tauri GUI app as usual.
    if let Some(first) = args.first() {
        if kansolo_lib::cli::SUBCOMMANDS.contains(&first.as_str()) {
            kansolo_lib::cli::dispatch_and_exit(&args);
        }
    }
    kansolo_lib::run()
}
