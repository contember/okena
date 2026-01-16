fn main() {
    // Windows: Embed icon into executable
    #[cfg(target_os = "windows")]
    {
        let icon_path = "assets/app-icon.ico";

        // Check if icon exists
        if std::path::Path::new(icon_path).exists() {
            let mut res = winresource::WindowsResource::new();
            res.set_icon(icon_path);
            res.set("FileDescription", "Term Manager");
            res.set("ProductName", "Term Manager");

            if let Err(e) = res.compile() {
                eprintln!("Warning: Failed to set Windows icon: {}", e);
            }
        } else {
            println!("cargo:warning=Icon file not found at {}, skipping Windows icon embedding", icon_path);
        }
    }

    // Rerun if icon changes
    println!("cargo:rerun-if-changed=assets/app-icon.ico");
    println!("cargo:rerun-if-changed=assets/app-icon-simple.svg");
}
