fn main() {
    match parley_health::hook::run() {
        Ok(()) => {}
        Err(_) => std::process::exit(1),
    }
}
