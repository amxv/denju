fn main() {
    let version = option_env!("DENJU_BUILD_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"));
    match std::env::args().nth(1).as_deref() {
        Some("--version" | "-V") => println!("denju {version}"),
        _ => println!("denju Rust scaffold ({version})"),
    }
}
