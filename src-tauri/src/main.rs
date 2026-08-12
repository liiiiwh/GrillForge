fn main() {
    detach_release_gui_console();
    grillforge_lib::run()
}

#[cfg(all(windows, not(debug_assertions)))]
fn detach_release_gui_console() {
    // Explorer-launched release GUI sessions do not need a console window.
    unsafe {
        windows_sys::Win32::System::Console::FreeConsole();
    }
}

#[cfg(not(all(windows, not(debug_assertions))))]
fn detach_release_gui_console() {}
