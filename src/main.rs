//! Transcribe Audio & Video to Text - Turn recordings, meetings and videos into text and subtitles on your own PC, with no time limit.
//!
//!   audio_to_text.exe                 open the app
//!   audio_to_text.exe --auto-update   internal: background update check/download
//!   audio_to_text.exe --shots <dir>   write screenshots of made-up scenes, then exit
//!                                 (add --theme dark for the dark look)

#![windows_subsystem = "windows"]

mod app;
mod settings;
mod shots;
mod theme;
mod update;

fn main() {
    // First thing, always: Velopack may need to run an install/uninstall/
    // update hook and exit before any of our own argument handling runs.
    update::run_app_hooks();

    let args: Vec<String> = std::env::args().skip(1).collect();

    // The detached child spawned below; must not spawn another.
    if args.first().map(String::as_str) == Some("--auto-update") {
        update::run_background_update();
        return;
    }
    if !args.iter().any(|a| a == "--shots") {
        update::maybe_spawn_background_update();
    }

    if let Err(e) = app::run(args) {
        eprintln!("{e}");
    }
}
