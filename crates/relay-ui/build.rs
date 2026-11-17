//! Embed the Relay icon and version metadata in Windows executables.

fn main() {
    println!("cargo:rerun-if-changed=../../packaging/icons/relay.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let mut resource = winresource::WindowsResource::new();
    resource.set_icon("../../packaging/icons/relay.ico");
    resource.set("ProductName", "Relay");
    resource.set(
        "FileDescription",
        "Relay — one conversation, every frontier model",
    );
    resource.set("CompanyName", "Relay contributors");
    resource.set("LegalCopyright", "Apache-2.0");
    resource.set("OriginalFilename", "relay.exe");
    resource.set("FileVersion", env!("CARGO_PKG_VERSION"));
    resource.set("ProductVersion", env!("CARGO_PKG_VERSION"));
    if let Err(error) = resource.compile() {
        println!("cargo:warning=could not embed Windows resources: {error}");
    }
}
