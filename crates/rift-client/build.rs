// Tells Cargo to rebuild the client whenever the
// RIFT_DEFAULT_SERVER env var changes. Without this, `option_env!`
// in main.rs would silently reuse the previous compile's value
// when the var flips between two `cargo build` invocations.
fn main() {
    println!("cargo:rerun-if-env-changed=RIFT_DEFAULT_SERVER");
    println!("cargo:rerun-if-env-changed=RIFT_DEV_AUTH_KEY");
}
