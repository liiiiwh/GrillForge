fn main() {
    match grillforge_lib::selector::run_cli(std::env::args_os().skip(1)) {
        Ok(Some(json)) => {
            println!("{json}");
            return;
        }
        Ok(None) => detach_release_gui_console(),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
    grillforge_lib::run()
}

#[cfg(all(windows, not(debug_assertions)))]
fn detach_release_gui_console() {
    // The single executable keeps a console subsystem so the bundled selector
    // CLI has reliable stdout/stderr. Explorer-launched release GUI sessions
    // detach that otherwise unused console before Tauri starts.
    unsafe {
        windows_sys::Win32::System::Console::FreeConsole();
    }
}

#[cfg(not(all(windows, not(debug_assertions))))]
fn detach_release_gui_console() {}
