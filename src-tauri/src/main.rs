fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("mcp-stdio") => {
            std::process::exit(match grillforge_lib::mcp_stdio::run_from_env() {
                Ok(()) => 0,
                Err(error) => {
                    eprintln!("GrillForge MCP stdio bridge stopped: {error}");
                    1
                }
            });
        }
        Some("claude-route-hook") => {
            std::process::exit(match grillforge_lib::claude_route_hook::run_from_env() {
                Ok(()) => 0,
                Err(error) => {
                    eprintln!("GrillForge Claude route hook stopped: {error}");
                    1
                }
            });
        }
        _ => {}
    }
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
