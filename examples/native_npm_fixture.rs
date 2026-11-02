//! Qualification-only bridge from native terminal harnesses to installed npm shims.
//! The Windows terminal fixture uses CreateProcess, which cannot launch .cmd.
use std::process::Command;
#[cfg(windows)]
use std::process::exit;

fn main() {
    let executable = std::env::current_exe().expect("fixture executable");
    let name = executable.file_stem().unwrap().to_str().unwrap();
    assert!(
        matches!(name, "twigsmith" | "tgi"),
        "stage the npm bridge under a command name"
    );
    let directory =
        std::env::var_os("TWIGSMITH_NPM_SHIM_DIR").expect("installed npm shim directory");
    let suffix = if cfg!(windows) { ".cmd" } else { "" };
    let shim = std::path::Path::new(&directory).join(format!("{name}{suffix}"));
    let mut command = Command::new(shim);
    command.args(std::env::args_os().skip(1));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        panic!("start the actual installed npm shim: {}", command.exec());
    }
    #[cfg(windows)]
    {
        // Console events reach the actual shim/Node/native process too. Let the
        // native handler finish recovery and return its status through the shim.
        unsafe extern "system" fn observe(_: u32) -> i32 {
            1
        }
        unsafe {
            windows_sys::Win32::System::Console::SetConsoleCtrlHandler(Some(observe), 1);
        }
        let status = command
            .status()
            .expect("start the actual installed npm shim");
        exit(status.code().unwrap_or(1));
    }
}
