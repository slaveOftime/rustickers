fn main() {
    #[cfg(target_os = "windows")]
    {
        // Embed the application icon into the Windows executable.
        // This controls the icon shown in Explorer / taskbar for the .exe.
        let mut res = winres::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.compile().expect("failed to compile Windows resources");
    }
}
