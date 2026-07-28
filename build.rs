fn main() {
    // Embed the application icon into the Windows executable. Tauri's bundler used to do
    // this; with a plain cargo build it has to be done explicitly or the .exe shows the
    // generic Windows icon in Explorer and the taskbar.
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=resources/icon.ico");
        winresource::WindowsResource::new()
            .set_icon("resources/icon.ico")
            .compile()
            .expect("failed to embed Windows resources");
    }
}
