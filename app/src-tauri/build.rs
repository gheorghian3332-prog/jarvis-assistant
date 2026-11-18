fn main() {
    let windows =
        tauri_build::WindowsAttributes::new().app_manifest(include_str!("jarvis.manifest"));
    let attributes = tauri_build::Attributes::new().windows_attributes(windows);
    if let Err(error) = tauri_build::try_build(attributes) {
        panic!("tauri build script failed: {error:#}");
    }
}
