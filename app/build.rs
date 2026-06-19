fn main() {
    println!("cargo:rerun-if-changed=favicon.ico");
    println!("cargo:rerun-if-changed=src/app_config.rs");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    if !std::path::Path::new("favicon.ico").is_file() {
        return;
    }

    let mut resource = winresource::WindowsResource::new();
    let app_config = std::fs::read_to_string("src/app_config.rs").unwrap_or_default();
    let app_identifier = const_str(&app_config, "APP_IDENTIFIER").unwrap_or_else(|| {
        std::env::var("CARGO_PKG_NAME").unwrap_or_else(|_| "webview-app".into())
    });
    let app_title = const_str(&app_config, "APP_TITLE")
        .unwrap_or_else(|| std::env::var("CARGO_PKG_DESCRIPTION").unwrap_or_default());
    let app_version = const_str(&app_config, "APP_VERSION")
        .unwrap_or_else(|| std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "1.0.0".into()));
    let exe_name = format!("{app_identifier}.exe");

    resource.set_icon("favicon.ico");
    resource
        .set("FileDescription", &app_title)
        .set("ProductName", &app_title)
        .set("InternalName", &exe_name)
        .set("OriginalFilename", &exe_name)
        .set("FileVersion", &app_version)
        .set("ProductVersion", &app_version);
    resource
        .compile()
        .expect("failed to compile Windows executable resources");
}

fn const_str(source: &str, name: &str) -> Option<String> {
    let prefix = format!("pub const {name}: &str = ");
    let line = source.lines().find(|line| line.starts_with(&prefix))?;
    let value = line.strip_prefix(&prefix)?.trim().strip_suffix(';')?.trim();
    let value = value.strip_prefix('"')?.strip_suffix('"')?;
    Some(value.replace("\\\"", "\"").replace("\\\\", "\\"))
}
