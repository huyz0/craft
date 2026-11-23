fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // Clap builds the full command tree before parsing even a single subcommand.
        let reserve = match std::env::var("CARGO_CFG_TARGET_ENV").as_deref() {
            Ok("msvc") => "/STACK:4194304",
            Ok("gnu") => "-Wl,--stack,4194304",
            _ => return,
        };
        println!("cargo:rustc-link-arg-bin=craft-factory={reserve}");
    }
}
