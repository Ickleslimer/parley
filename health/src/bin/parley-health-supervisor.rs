fn main() {
    if let Err(error) = parley_health::supervisor::run_forever() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
