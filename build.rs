fn main() {
    // Ensure web/dist/ exists so rust-embed compiles even without a pre-built web client
    let web_dist = std::path::Path::new("web/dist");
    if !web_dist.exists() {
        std::fs::create_dir_all(web_dist).expect("failed to create web/dist directory");
        // Create a minimal placeholder index.html
        std::fs::write(
            web_dist.join("index.html"),
            "<html><body>Web client not built. Run: cd web && bun install && bun run build</body></html>",
        )
        .expect("failed to write placeholder index.html");
    }
    println!("cargo:rerun-if-changed=web/dist");

    // Windows: Embed icon into executable
    #[cfg(target_os = "windows")]
    {
        let icon_path = "assets/app-icon.ico";

        // Check if icon exists
        if std::path::Path::new(icon_path).exists() {
            let mut res = winresource::WindowsResource::new();
            res.set_icon(icon_path);
            res.set("FileDescription", "Okena");
            res.set("ProductName", "Okena");

            if let Err(e) = res.compile() {
                eprintln!("Warning: Failed to set Windows icon: {}", e);
            }
        } else {
            println!("cargo:warning=Icon file not found at {}, skipping Windows icon embedding", icon_path);
        }
    }

    // Rerun if icon changes
    println!("cargo:rerun-if-changed=assets/app-icon.ico");
}
