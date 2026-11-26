fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    // Vendored OpenSSL does not provide its original PDB
    // alongside the linked static library.
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg=/IGNORE:4099");
    }
}
