use std::path::PathBuf;

use parley_health::integration;
use parley_health::paths::HealthPaths;
use parley_health::schema::HealthError;

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), HealthError> {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if args.is_empty() {
        return parley_health::supervisor::run_forever();
    }
    let mode = args[0].to_string_lossy();
    let install_dir = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(PathBuf::from))
        .ok_or_else(|| HealthError::msg("cannot resolve health installation directory"))?;
    match mode.as_ref() {
        "--configure-r3" if args.len() == 3 => integration::configure_r3(
            &HealthPaths::from_env(),
            &PathBuf::from(&args[1]),
            &PathBuf::from(&args[2]),
            &install_dir,
            &integration::grok_home(),
        ),
        "--refresh-installation" if args.len() == 1 => integration::refresh_installation(
            &HealthPaths::from_env(),
            &install_dir,
            &integration::grok_home(),
        ),
        "--remove-hooks" if args.len() == 1 => integration::remove_hooks(&integration::grok_home())
            .map(|_| ()),
        "--shutdown" if args.len() == 1 => parley_health::instance::signal_shutdown(
            parley_health::instance::SUPERVISOR_SHUTDOWN_EVENT,
        ),
        _ => Err(HealthError::msg(
            "usage: parley-health-supervisor.exe [--configure-r3 <git-common-dir> <main-root> | --refresh-installation | --remove-hooks | --shutdown]",
        )),
    }
}
