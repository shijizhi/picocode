fn main() {
    if let Err(error) = picocode::run_cli(std::env::args().skip(1)) {
        let _ = crossterm::terminal::disable_raw_mode();
        eprintln!("picocode error: {error}");
        std::process::exit(1);
    }
}
