#[cfg(windows)]
fn main() {
    let manifest_dir = std::path::PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is not set"),
    );
    let icon = manifest_dir.join("assets/icon.ico");

    println!("cargo:rerun-if-changed=assets/icon.ico");
    winresource::WindowsResource::new()
        .set_icon(icon.to_str().expect("icon path is not valid UTF-8"))
        .compile()
        .expect("failed to compile Windows executable icon");
}

#[cfg(not(windows))]
fn main() {}
