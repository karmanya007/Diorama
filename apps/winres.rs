//! Shared by the build scripts of dioramad, diorama-webhost and diorama-cli:
//! embeds Windows version information (product name, version, description)
//! and the Diorama icon, as diorama-ui gets from Tauri. Explorer and Task
//! Manager show them, and code signing through the SignPath Foundation
//! requires the product name and version on every signed program.

const ICON: &str = "../diorama-ui/src-tauri/icons/icon.ico";

/// `description` is the FileDescription, the name Task Manager lists.
pub fn embed(description: &str) {
    println!("cargo:rerun-if-changed={ICON}");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        compile(description);
    }
}

#[cfg(windows)]
fn compile(description: &str) {
    let mut res = tauri_winres::WindowsResource::new();
    res.set_icon(ICON)
        .set("ProductName", "Diorama")
        .set("FileDescription", description)
        .set("CompanyName", "Diorama")
        .set("LegalCopyright", "Diorama contributors");
    if let Err(e) = res.compile() {
        println!("cargo::error=embedding Windows resources failed: {e}");
    }
}

#[cfg(not(windows))]
fn compile(_description: &str) {}
