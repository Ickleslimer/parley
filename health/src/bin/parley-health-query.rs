fn main() {
    let extra: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let result = parley_health::query::run(&extra);
    println!("{}", result.json);
    std::process::exit(result.exit_code);
}
