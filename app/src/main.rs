#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

#[cfg(target_os = "windows")]
mod app_config;
#[cfg(all(target_os = "windows", feature = "reuse-instance"))]
mod single_instance;
#[cfg(all(target_os = "windows", feature = "notifications"))]
mod windows_notifications;

#[cfg(target_os = "windows")]
mod windows_app {
    use super::app_config::{
        APP_CLOSE_TO_TRAY, APP_IDENTIFIER, APP_NOTIFICATIONS, APP_TITLE, APP_TRAY_ICON, APP_URLS,
        APP_VERSION, INTERNAL_URL_PREFIXES, INTERNAL_URL_REGEXES, MAILTO_URL_TEMPLATE,
        WEBVIEW_ARGS, WEBVIEW_INCOGNITO, WINDOW_ALLOW_NEW, WINDOW_ALWAYS_ON_TOP, WINDOW_DRAG_DROP,
        WINDOW_FULLSCREEN, WINDOW_HEIGHT, WINDOW_MAXIMIZED, WINDOW_RESIZABLE, WINDOW_TITLE_BAR,
        WINDOW_WIDTH,
    };
    #[cfg(feature = "reuse-instance")]
    use super::single_instance::{self, InstanceClaim};
    #[cfg(feature = "regex-internal-urls")]
    use regex_lite::Regex;
    #[cfg(feature = "use-favicon")]
    use std::sync::Mutex;
    use std::{
        collections::HashMap,
        env,
        path::PathBuf,
        process,
        sync::{
            OnceLock,
            atomic::{AtomicUsize, Ordering},
        },
    };
    #[cfg(feature = "use-favicon")]
    use std::{
        collections::hash_map::DefaultHasher,
        hash::{Hash, Hasher},
    };
    use tao::{
        dpi::{LogicalSize, PhysicalSize},
        event::{Event, WindowEvent},
        event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy, EventLoopWindowTarget},
        platform::windows::{EventLoopBuilderExtWindows, IconExtWindows, WindowBuilderExtWindows},
        window::{Fullscreen, Icon, Theme, Window, WindowBuilder, WindowId},
    };
    #[cfg(feature = "tray-icon")]
    use tray_icon::{
        MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
        menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    };
    #[cfg(feature = "use-favicon")]
    use webview2_com::{
        FaviconChangedEventHandler, GetFaviconCompletedHandler,
        Microsoft::Web::WebView2::Win32::{
            COREWEBVIEW2_FAVICON_IMAGE_FORMAT_PNG, ICoreWebView2_15,
        },
    };
    #[cfg(feature = "notifications")]
    use webview2_com::{
        Microsoft::Web::WebView2::Win32::{
            COREWEBVIEW2_PERMISSION_KIND, COREWEBVIEW2_PERMISSION_KIND_NOTIFICATIONS,
            COREWEBVIEW2_PERMISSION_STATE_ALLOW, COREWEBVIEW2_PERMISSION_STATE_DENY,
            ICoreWebView2_13, ICoreWebView2_24, ICoreWebView2PermissionRequestedEventArgs3,
            ICoreWebView2Profile4,
        },
        NotificationReceivedEventHandler, PermissionRequestedEventHandler,
        SetPermissionStateCompletedHandler,
    };
    #[cfg(feature = "notifications")]
    use windows::Win32::System::Com::CoTaskMemFree;
    #[cfg(feature = "use-favicon")]
    use windows::Win32::{
        Graphics::Imaging::{
            CLSID_WICImagingFactory, GUID_WICPixelFormat32bppRGBA, IWICImagingFactory,
            WICBitmapDitherTypeNone, WICBitmapPaletteTypeCustom, WICDecodeMetadataCacheOnLoad,
        },
        System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IStream, STREAM_SEEK_SET},
    };
    #[cfg(any(feature = "notifications", feature = "use-favicon"))]
    use windows::core::Interface;
    #[cfg(feature = "notifications")]
    use windows::core::{HSTRING, PWSTR};
    use windows_sys::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL};
    #[cfg(any(feature = "notifications", feature = "use-favicon"))]
    use wry::WebViewExtWindows;
    use wry::{
        NewWindowFeatures, NewWindowResponse, WebContext, WebView, WebViewBuilder,
        WebViewBuilderExtWindows,
    };

    static WINDOW_COUNTER: AtomicUsize = AtomicUsize::new(1);
    #[cfg(feature = "regex-internal-urls")]
    static INTERNAL_REGEXES: OnceLock<Vec<Regex>> = OnceLock::new();
    static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();
    #[cfg(feature = "use-favicon")]
    static WINDOW_FAVICON_PATHS: OnceLock<Mutex<HashMap<WindowId, PathBuf>>> = OnceLock::new();

    const APP_ICON_RESOURCE_ID: u16 = 1;

    enum UserEvent {
        #[cfg(feature = "reuse-instance")]
        OpenArg(Option<String>),
        NewWindow(String, Option<LogicalSize<f64>>),
        NewTitle(WindowId, String),
        #[cfg(feature = "use-favicon")]
        Favicon(WindowId, Icon, PathBuf),
        #[cfg(feature = "tray-icon")]
        TrayIconEvent(TrayIconEvent),
        #[cfg(feature = "tray-icon")]
        MenuEvent(MenuEvent),
    }

    struct AppWindow {
        window: Window,
        _webview: WebView,
    }

    #[cfg(feature = "tray-icon")]
    struct AppTray {
        _tray: TrayIcon,
        new_window_id: Option<String>,
        quit_id: Option<String>,
    }

    #[cfg(not(feature = "tray-icon"))]
    struct AppTray;

    #[cfg(feature = "regex-internal-urls")]
    fn internal_regexes() -> &'static [Regex] {
        INTERNAL_REGEXES
            .get_or_init(|| {
                INTERNAL_URL_REGEXES
                    .iter()
                    .filter_map(|pattern| Regex::new(pattern).ok())
                    .collect()
            })
            .as_slice()
    }

    fn is_internal_url(url: &str) -> bool {
        if APP_URLS.iter().any(|(_, prefix)| url.starts_with(prefix))
            || INTERNAL_URL_PREFIXES
                .iter()
                .any(|prefix| url.starts_with(prefix))
        {
            return true;
        }

        #[cfg(feature = "regex-internal-urls")]
        {
            return internal_regexes()
                .iter()
                .any(|pattern| pattern.is_match(url));
        }

        #[cfg(not(feature = "regex-internal-urls"))]
        {
            let _ = INTERNAL_URL_REGEXES;
            false
        }
    }

    #[cfg(feature = "notifications")]
    fn matches_internal_url_regex(url: &str) -> bool {
        #[cfg(feature = "regex-internal-urls")]
        {
            return internal_regexes()
                .iter()
                .any(|pattern| pattern.is_match(url));
        }

        #[cfg(not(feature = "regex-internal-urls"))]
        {
            let _ = url;
            let _ = INTERNAL_URL_REGEXES;
            false
        }
    }

    #[cfg(feature = "notifications")]
    fn is_listed_internal_url(url: &str) -> bool {
        APP_URLS.iter().any(|(_, prefix)| url.starts_with(prefix))
            || INTERNAL_URL_PREFIXES
                .iter()
                .any(|prefix| url.starts_with(prefix))
    }

    #[cfg(feature = "notifications")]
    fn allows_notification_permission(url: &str) -> bool {
        is_listed_internal_url(url) || matches_internal_url_regex(url)
    }

    #[cfg(feature = "notifications")]
    fn origin_from_url(url: &str) -> Option<String> {
        let (scheme, rest) = url.split_once("://")?;
        if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
            return None;
        }

        let authority = rest.split(['/', '?', '#']).next()?;
        if authority.is_empty() {
            return None;
        }

        Some(format!("{}://{authority}/", scheme.to_ascii_lowercase()))
    }

    #[cfg(feature = "notifications")]
    fn listed_notification_origins() -> Vec<String> {
        let mut origins = Vec::new();
        for url in APP_URLS
            .iter()
            .map(|(_, url)| *url)
            .chain(INTERNAL_URL_PREFIXES.iter().copied())
        {
            let Some(origin) = origin_from_url(url) else {
                continue;
            };
            if !origins.iter().any(|existing| existing == &origin) {
                origins.push(origin);
            }
        }
        origins
    }

    #[cfg(feature = "notifications")]
    fn take_pwstr(value: PWSTR) -> String {
        if value.is_null() {
            return String::new();
        }

        unsafe {
            let text = value.to_string().unwrap_or_default();
            CoTaskMemFree(Some(value.as_ptr() as _));
            text
        }
    }

    #[cfg(all(feature = "notifications", feature = "use-favicon"))]
    fn notification_icon_path(window_id: WindowId) -> Option<PathBuf> {
        WINDOW_FAVICON_PATHS
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .ok()
            .and_then(|paths| paths.get(&window_id).cloned())
    }

    #[cfg(feature = "use-favicon")]
    fn set_window_favicon_path(window_id: WindowId, path: PathBuf) {
        if let Ok(mut paths) = WINDOW_FAVICON_PATHS
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
        {
            paths.insert(window_id, path);
        }
    }

    #[cfg(feature = "use-favicon")]
    fn read_stream(stream: &IStream) -> Option<Vec<u8>> {
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 8192];

        loop {
            let mut read = 0_u32;
            let result = unsafe {
                stream.Read(
                    buffer.as_mut_ptr().cast(),
                    buffer.len() as u32,
                    Some(&mut read),
                )
            };
            if result.is_err() {
                return None;
            }
            if read == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..read as usize]);
        }

        Some(bytes)
    }

    #[cfg(feature = "use-favicon")]
    fn decode_favicon_rgba(stream: &IStream) -> Option<(Vec<u8>, u32, u32)> {
        unsafe {
            let _ = stream.Seek(0, STREAM_SEEK_SET, None);

            let factory: IWICImagingFactory =
                CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
            let decoder = factory
                .CreateDecoderFromStream(stream, std::ptr::null(), WICDecodeMetadataCacheOnLoad)
                .ok()?;
            let frame = decoder.GetFrame(0).ok()?;
            let converter = factory.CreateFormatConverter().ok()?;
            converter
                .Initialize(
                    &frame,
                    &GUID_WICPixelFormat32bppRGBA,
                    WICBitmapDitherTypeNone,
                    None,
                    0.0,
                    WICBitmapPaletteTypeCustom,
                )
                .ok()?;

            let mut width = 0;
            let mut height = 0;
            converter.GetSize(&mut width, &mut height).ok()?;
            if width == 0 || height == 0 || width > 512 || height > 512 {
                return None;
            }

            let stride = width.checked_mul(4)?;
            let len = stride.checked_mul(height)? as usize;
            let mut rgba = vec![0; len];
            converter
                .CopyPixels(std::ptr::null(), stride, rgba.as_mut_slice())
                .ok()?;

            Some((rgba, width, height))
        }
    }

    #[cfg(feature = "use-favicon")]
    fn favicon_path(window_id: WindowId) -> PathBuf {
        data_dir()
            .join("favicons")
            .join(format!("{:016x}.png", window_id_hash(window_id)))
    }

    #[cfg(feature = "use-favicon")]
    fn window_id_hash(window_id: WindowId) -> u64 {
        let mut hasher = DefaultHasher::new();
        window_id.hash(&mut hasher);
        hasher.finish()
    }

    #[cfg(feature = "use-favicon")]
    fn handle_favicon_stream(
        stream: &IStream,
        window_id: WindowId,
        proxy: &EventLoopProxy<UserEvent>,
    ) {
        let Some(png) = read_stream(stream) else {
            return;
        };
        let _ = unsafe { stream.Seek(0, STREAM_SEEK_SET, None) };
        let Some((rgba, width, height)) = decode_favicon_rgba(stream) else {
            return;
        };
        let Ok(icon) = Icon::from_rgba(rgba, width, height) else {
            return;
        };

        let path = favicon_path(window_id);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if std::fs::write(&path, png).is_ok() {
            let _ = proxy.send_event(UserEvent::Favicon(window_id, icon, path));
        }
    }

    #[cfg(feature = "use-favicon")]
    fn configure_favicon_updates(
        window_id: WindowId,
        webview: &WebView,
        proxy: EventLoopProxy<UserEvent>,
    ) {
        let core = webview.webview();
        let Ok(webview15) = core.cast::<ICoreWebView2_15>() else {
            return;
        };

        unsafe {
            let mut token = 0;
            let changed_proxy = proxy.clone();
            let _ = webview15.add_FaviconChanged(
                &FaviconChangedEventHandler::create(Box::new(move |webview, _| {
                    let Some(webview) = webview else {
                        return Ok(());
                    };
                    let Ok(webview15) = webview.cast::<ICoreWebView2_15>() else {
                        return Ok(());
                    };

                    let proxy = changed_proxy.clone();
                    let _ = webview15.GetFavicon(
                        COREWEBVIEW2_FAVICON_IMAGE_FORMAT_PNG,
                        &GetFaviconCompletedHandler::create(Box::new(move |error, stream| {
                            if error.is_ok() {
                                if let Some(stream) = stream.as_ref() {
                                    handle_favicon_stream(stream, window_id, &proxy);
                                }
                            }
                            Ok(())
                        })),
                    );

                    Ok(())
                })),
                &mut token,
            );

            let proxy = proxy.clone();
            let _ = webview15.GetFavicon(
                COREWEBVIEW2_FAVICON_IMAGE_FORMAT_PNG,
                &GetFaviconCompletedHandler::create(Box::new(move |error, stream| {
                    if error.is_ok() {
                        if let Some(stream) = stream.as_ref() {
                            handle_favicon_stream(stream, window_id, &proxy);
                        }
                    }
                    Ok(())
                })),
            );
        }
    }

    #[cfg(feature = "notifications")]
    fn configure_html5_notifications(window_id: WindowId, webview: &WebView) {
        if !APP_NOTIFICATIONS {
            return;
        }

        let core = webview.webview();

        unsafe {
            let mut token = 0;
            let _ = core.add_PermissionRequested(
                &PermissionRequestedEventHandler::create(Box::new(|_, args| {
                    let Some(args) = args else {
                        return Ok(());
                    };

                    let mut kind = COREWEBVIEW2_PERMISSION_KIND::default();
                    args.PermissionKind(&mut kind)?;
                    if kind != COREWEBVIEW2_PERMISSION_KIND_NOTIFICATIONS {
                        return Ok(());
                    }

                    let uri = {
                        let mut uri = PWSTR::null();
                        args.Uri(&mut uri)?;
                        take_pwstr(uri)
                    };

                    if allows_notification_permission(&uri) {
                        if let Ok(args3) = args.cast::<ICoreWebView2PermissionRequestedEventArgs3>()
                        {
                            let _ = args3.SetSavesInProfile(true);
                        }
                        args.SetState(COREWEBVIEW2_PERMISSION_STATE_ALLOW)?;
                    } else {
                        args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY)?;
                    }

                    Ok(())
                })),
                &mut token,
            );

            if let Ok(webview24) = core.cast::<ICoreWebView2_24>() {
                let _ = webview24.add_NotificationReceived(
                    &NotificationReceivedEventHandler::create(Box::new(move |_, args| {
                        let Some(args) = args else {
                            return Ok(());
                        };

                        let notification = args.Notification()?;
                        let title = {
                            let mut title = PWSTR::null();
                            notification.Title(&mut title)?;
                            take_pwstr(title)
                        };
                        let body = {
                            let mut body = PWSTR::null();
                            notification.Body(&mut body)?;
                            take_pwstr(body)
                        };

                        args.SetHandled(true)?;
                        notify_web(window_id, &title, &body);
                        let _ = notification.ReportShown();

                        Ok(())
                    })),
                    &mut token,
                );
            }

            let Ok(profile) = core
                .cast::<ICoreWebView2_13>()
                .and_then(|webview| webview.Profile())
                .and_then(|profile| profile.cast::<ICoreWebView2Profile4>())
            else {
                return;
            };

            for origin in listed_notification_origins() {
                let origin = HSTRING::from(origin);
                let handler = SetPermissionStateCompletedHandler::create(Box::new(|_| Ok(())));
                let _ = profile.SetPermissionState(
                    COREWEBVIEW2_PERMISSION_KIND_NOTIFICATIONS,
                    &origin,
                    COREWEBVIEW2_PERMISSION_STATE_ALLOW,
                    &handler,
                );
            }
        }
    }

    fn open_external(url: &str) {
        let open = wide_null("open");
        let url = wide_null(url);
        unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                open.as_ptr(),
                url.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                SW_SHOWNORMAL,
            );
        }
    }

    fn wide_null(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn disable_drag_drop_script() -> &'static str {
        r#"
(() => {
  const block = event => {
    event.preventDefault();
    event.stopPropagation();
  };
  window.addEventListener('dragstart', block, true);
  window.addEventListener('dragenter', block, true);
  window.addEventListener('dragover', block, true);
  window.addEventListener('drop', block, true);
})();
"#
    }

    fn percent_encode(value: &str) -> String {
        const HEX: &[u8; 16] = b"0123456789ABCDEF";
        let mut encoded = String::with_capacity(value.len());

        for byte in value.bytes() {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
                encoded.push(byte as char);
            } else {
                encoded.push('%');
                encoded.push(HEX[(byte >> 4) as usize] as char);
                encoded.push(HEX[(byte & 0x0f) as usize] as char);
            }
        }

        encoded
    }

    fn mailto_url(mailto: &str) -> Option<String> {
        let mailto = mailto.trim();
        if !mailto
            .get(..7)
            .is_some_and(|scheme| scheme.eq_ignore_ascii_case("mailto:"))
        {
            return None;
        }

        let encoded = percent_encode(mailto);
        Some(if MAILTO_URL_TEMPLATE.contains("{}") {
            MAILTO_URL_TEMPLATE.replace("{}", &encoded)
        } else if MAILTO_URL_TEMPLATE.contains("%s") {
            MAILTO_URL_TEMPLATE.replace("%s", &encoded)
        } else {
            format!("{MAILTO_URL_TEMPLATE}{encoded}")
        })
    }

    fn next_window_label() -> String {
        let id = WINDOW_COUNTER.fetch_add(1, Ordering::Relaxed);
        format!("window-{id}")
    }

    fn data_dir_prefix() -> String {
        let app = APP_IDENTIFIER
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect::<String>();
        format!("{app}-")
    }

    fn data_dir() -> PathBuf {
        DATA_DIR
            .get_or_init(|| {
                if WEBVIEW_INCOGNITO {
                    env::temp_dir().join(format!("{}{}", data_dir_prefix(), process::id()))
                } else {
                    let app = APP_IDENTIFIER
                        .chars()
                        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                        .collect::<String>();

                    dirs::data_local_dir()
                        .unwrap_or_else(env::temp_dir)
                        .join("nest")
                        .join(app)
                }
            })
            .clone()
    }

    #[cfg(feature = "notifications")]
    fn install_windows_notifications() {
        if APP_NOTIFICATIONS {
            super::windows_notifications::install(APP_IDENTIFIER, APP_TITLE);
        }
    }

    #[cfg(not(feature = "notifications"))]
    fn install_windows_notifications() {
        let _ = APP_NOTIFICATIONS;
    }

    #[cfg(feature = "notifications")]
    fn notify_download_finished(window_id: WindowId, path: Option<&PathBuf>, success: bool) {
        if !APP_NOTIFICATIONS {
            return;
        }
        #[cfg(not(feature = "use-favicon"))]
        let _ = window_id;

        let (title, body, download_path) = if success {
            (
                "Download completed",
                path.map(|path| format!("Saved to {}", path.display()))
                    .unwrap_or_else(|| "Download completed".to_string()),
                path.map(PathBuf::as_path),
            )
        } else {
            ("Download failed", "Download failed".to_string(), None)
        };
        #[cfg(feature = "use-favicon")]
        let icon_path = notification_icon_path(window_id);
        #[cfg(not(feature = "use-favicon"))]
        let icon_path: Option<PathBuf> = None;
        super::windows_notifications::notify(
            APP_IDENTIFIER,
            title,
            &body,
            download_path,
            icon_path.as_deref(),
        );
    }

    #[cfg(feature = "notifications")]
    fn notify_web(window_id: WindowId, title: &str, body: &str) {
        if APP_NOTIFICATIONS {
            #[cfg(not(feature = "use-favicon"))]
            let _ = window_id;
            #[cfg(feature = "use-favicon")]
            let icon_path = notification_icon_path(window_id);
            #[cfg(not(feature = "use-favicon"))]
            let icon_path: Option<PathBuf> = None;
            super::windows_notifications::notify(
                APP_IDENTIFIER,
                title,
                body,
                None,
                icon_path.as_deref(),
            );
        }
    }

    fn app_url_for_selection(selection: Option<&str>) -> &'static str {
        let default_url = APP_URLS
            .first()
            .map(|(_, url)| *url)
            .unwrap_or("https://example.com/");

        let Some(selection) = selection else {
            return default_url;
        };

        APP_URLS
            .iter()
            .find_map(|(name, url)| (*name == selection).then_some(*url))
            .unwrap_or(default_url)
    }

    #[cfg(feature = "tray-icon")]
    fn selected_app_url() -> &'static str {
        app_url_for_selection(env::args().nth(1).as_deref())
    }

    fn start_url_from_arg(arg: Option<&str>) -> String {
        arg.and_then(mailto_url)
            .unwrap_or_else(|| app_url_for_selection(arg).to_string())
    }

    fn selected_start_url() -> String {
        start_url_from_arg(env::args().nth(1).as_deref())
    }

    #[cfg(feature = "tray-icon")]
    fn tray_icon() -> Option<tray_icon::Icon> {
        tray_icon::Icon::from_resource(APP_ICON_RESOURCE_ID, Some((32, 32))).ok()
    }

    fn window_icon() -> Option<Icon> {
        Icon::from_resource(APP_ICON_RESOURCE_ID, Some(PhysicalSize::new(32, 32))).ok()
    }

    #[cfg(feature = "reuse-instance")]
    fn install_reuse_instance_listener(proxy: EventLoopProxy<UserEvent>) {
        if !super::app_config::APP_REUSE_INSTANCE {
            return;
        }

        single_instance::listen(APP_IDENTIFIER, move |arg| {
            let _ = proxy.send_event(UserEvent::OpenArg((!arg.is_empty()).then_some(arg)));
        });
    }

    #[cfg(not(feature = "reuse-instance"))]
    fn install_reuse_instance_listener(_proxy: EventLoopProxy<UserEvent>) {}

    #[cfg(feature = "tray-icon")]
    fn install_tray(proxy: EventLoopProxy<UserEvent>) -> Option<AppTray> {
        if !APP_TRAY_ICON {
            return None;
        }

        let mut tray_builder = TrayIconBuilder::new().with_tooltip(APP_TITLE);
        if let Some(icon) = tray_icon() {
            tray_builder = tray_builder.with_icon(icon);
        }

        let menu = Menu::new();
        let new_window = MenuItem::new("New Window", true, None);
        let quit = MenuItem::new("Quit", true, None);
        let separator = PredefinedMenuItem::separator();
        let _ = menu.append(&new_window);
        let _ = menu.append(&separator);
        let _ = menu.append(&quit);
        let new_window_id = Some(new_window.id().0.clone());
        let quit_id = Some(quit.id().0.clone());
        tray_builder = tray_builder.with_menu(Box::new(menu));

        let tray_proxy = proxy.clone();
        TrayIconEvent::set_event_handler(Some(move |event| {
            let _ = tray_proxy.send_event(UserEvent::TrayIconEvent(event));
        }));

        MenuEvent::set_event_handler(Some(move |event| {
            let _ = proxy.send_event(UserEvent::MenuEvent(event));
        }));

        tray_builder.build().ok().map(|tray| AppTray {
            _tray: tray,
            new_window_id,
            quit_id,
        })
    }

    #[cfg(not(feature = "tray-icon"))]
    fn install_tray(_proxy: EventLoopProxy<UserEvent>) -> Option<AppTray> {
        let _ = APP_CLOSE_TO_TRAY;
        let _ = APP_TRAY_ICON;
        None
    }

    #[cfg(feature = "tray-icon")]
    fn keep_alive_on_close() -> bool {
        APP_CLOSE_TO_TRAY && APP_TRAY_ICON
    }

    #[cfg(not(feature = "tray-icon"))]
    fn keep_alive_on_close() -> bool {
        false
    }

    fn build_window(
        event_loop: &EventLoopWindowTarget<UserEvent>,
        context: &mut WebContext,
        proxy: EventLoopProxy<UserEvent>,
        label: String,
        url: String,
        size: Option<LogicalSize<f64>>,
        visible: bool,
    ) -> Result<AppWindow, String> {
        let fullscreen = WINDOW_FULLSCREEN.then_some(Fullscreen::Borderless(None));
        let window_builder = WindowBuilder::new()
            .with_title(APP_TITLE)
            .with_window_icon(window_icon())
            .with_inner_size(size.unwrap_or_else(|| LogicalSize::new(WINDOW_WIDTH, WINDOW_HEIGHT)))
            .with_resizable(WINDOW_RESIZABLE)
            .with_decorations(WINDOW_TITLE_BAR)
            .with_fullscreen(fullscreen)
            .with_maximized(WINDOW_MAXIMIZED)
            .with_always_on_top(WINDOW_ALWAYS_ON_TOP)
            .with_theme(Some(Theme::Dark))
            .with_drag_and_drop(WINDOW_DRAG_DROP)
            .with_visible(visible);

        let window = window_builder
            .build(event_loop)
            .map_err(|err| err.to_string())?;
        let window_id = window.id();

        let nav_handler = |url: String| {
            if is_internal_url(&url) {
                true
            } else {
                open_external(&url);
                false
            }
        };

        let new_window_proxy = proxy.clone();
        let new_window_handler = move |url: String, features: NewWindowFeatures| {
            if !WINDOW_ALLOW_NEW {
                return NewWindowResponse::Deny;
            }

            if !is_internal_url(&url) {
                open_external(&url);
                return NewWindowResponse::Deny;
            }

            let _ = new_window_proxy.send_event(UserEvent::NewWindow(url, features.size));
            NewWindowResponse::Deny
        };

        let title_proxy = proxy.clone();
        let title_handler = move |title: String| {
            let _ = title_proxy.send_event(UserEvent::NewTitle(window_id, title));
        };

        let mut webview_builder = WebViewBuilder::new_with_web_context(context)
            .with_id(Box::leak(label.into_boxed_str()))
            .with_url(url)
            .with_incognito(WEBVIEW_INCOGNITO)
            .with_navigation_handler(nav_handler)
            .with_new_window_req_handler(new_window_handler)
            .with_document_title_changed_handler(title_handler);

        #[cfg(feature = "notifications")]
        {
            if APP_NOTIFICATIONS {
                webview_builder = webview_builder
                    .with_download_started_handler(|_, _| true)
                    .with_download_completed_handler(move |_, path, success| {
                        notify_download_finished(window_id, path.as_ref(), success);
                    });
            }
        }

        if !WEBVIEW_ARGS.is_empty() {
            webview_builder = webview_builder.with_additional_browser_args(WEBVIEW_ARGS);
        }
        if !WINDOW_DRAG_DROP {
            webview_builder = webview_builder
                .with_initialization_script(disable_drag_drop_script())
                .with_drag_drop_handler(|_| true);
        }

        let webview = webview_builder
            .build(&window)
            .map_err(|err| err.to_string())?;
        #[cfg(feature = "notifications")]
        configure_html5_notifications(window_id, &webview);
        #[cfg(feature = "use-favicon")]
        configure_favicon_updates(window_id, &webview, proxy.clone());

        Ok(AppWindow {
            window,
            _webview: webview,
        })
    }

    fn build_profile_keeper(
        event_loop: &EventLoopWindowTarget<UserEvent>,
        context: &mut WebContext,
    ) -> Result<AppWindow, String> {
        let window = WindowBuilder::new()
            .with_title(APP_TITLE)
            .with_inner_size(LogicalSize::new(1.0, 1.0))
            .with_visible(false)
            .with_decorations(false)
            .with_skip_taskbar(true)
            .build(event_loop)
            .map_err(|err| err.to_string())?;

        let mut webview_builder = WebViewBuilder::new_with_web_context(context)
            .with_id("__profile_keeper")
            .with_url("about:blank")
            .with_incognito(WEBVIEW_INCOGNITO);

        if !WEBVIEW_ARGS.is_empty() {
            webview_builder = webview_builder.with_additional_browser_args(WEBVIEW_ARGS);
        }
        if !WINDOW_DRAG_DROP {
            webview_builder = webview_builder
                .with_initialization_script(disable_drag_drop_script())
                .with_drag_drop_handler(|_| true);
        }

        let webview = webview_builder
            .build(&window)
            .map_err(|err| err.to_string())?;
        #[cfg(feature = "notifications")]
        configure_html5_notifications(window.id(), &webview);

        Ok(AppWindow {
            window,
            _webview: webview,
        })
    }

    fn show_window_front(window: &Window) {
        window.set_visible(true);
        if window.is_minimized() {
            window.set_minimized(false);
        }
        window.set_focus();
    }

    #[cfg(feature = "tray-icon")]
    fn activate_existing_windows_or_create(
        windows: &mut HashMap<WindowId, AppWindow>,
        event_loop: &EventLoopWindowTarget<UserEvent>,
        context: &mut WebContext,
        proxy: EventLoopProxy<UserEvent>,
    ) {
        if windows.is_empty() {
            if let Ok(window) = build_window(
                event_loop,
                context,
                proxy,
                next_window_label(),
                selected_app_url().to_string(),
                None,
                true,
            ) {
                windows.insert(window.window.id(), window);
            }
            return;
        }

        for app_window in windows.values() {
            show_window_front(&app_window.window);
        }
    }

    pub fn main() {
        let _ = APP_VERSION;
        install_windows_notifications();

        #[cfg(feature = "reuse-instance")]
        let first_arg = env::args().nth(1);
        #[cfg(feature = "reuse-instance")]
        let instance_lock = match single_instance::claim_primary(
            APP_IDENTIFIER,
            super::app_config::APP_REUSE_INSTANCE,
        ) {
            InstanceClaim::Disabled | InstanceClaim::Failed => None,
            InstanceClaim::Primary(lock) => Some(lock),
            InstanceClaim::Secondary => {
                let _ = single_instance::send_to_primary(APP_IDENTIFIER, first_arg.as_deref());
                return;
            }
        };
        #[cfg(not(feature = "reuse-instance"))]
        let instance_lock = {
            let _ = super::app_config::APP_REUSE_INSTANCE;
        };

        let mut event_loop_builder = EventLoopBuilder::<UserEvent>::with_user_event();
        event_loop_builder.with_theme(Some(Theme::Dark));
        let event_loop = event_loop_builder.build();
        let proxy = event_loop.create_proxy();
        let mut web_context = WebContext::new(Some(data_dir()));

        install_reuse_instance_listener(proxy.clone());
        #[cfg(feature = "tray-icon")]
        let tray = install_tray(proxy.clone());
        #[cfg(not(feature = "tray-icon"))]
        install_tray(proxy.clone());

        let mut windows = HashMap::new();
        let _profile_keeper = (WEBVIEW_INCOGNITO && keep_alive_on_close())
            .then(|| build_profile_keeper(&event_loop, &mut web_context).ok())
            .flatten();
        if let Ok(window) = build_window(
            &event_loop,
            &mut web_context,
            proxy.clone(),
            "main".into(),
            selected_start_url(),
            None,
            true,
        ) {
            windows.insert(window.window.id(), window);
        }

        event_loop.run(move |event, event_loop, control_flow| {
            let _ = &instance_lock;
            *control_flow = ControlFlow::Wait;

            match event {
                Event::WindowEvent {
                    event: WindowEvent::CloseRequested,
                    window_id,
                    ..
                } => {
                    windows.remove(&window_id);
                    if windows.is_empty() && !keep_alive_on_close() {
                        *control_flow = ControlFlow::Exit;
                    }
                }
                #[cfg(feature = "reuse-instance")]
                Event::UserEvent(UserEvent::OpenArg(arg)) => {
                    if let Ok(window) = build_window(
                        event_loop,
                        &mut web_context,
                        proxy.clone(),
                        next_window_label(),
                        start_url_from_arg(arg.as_deref()),
                        None,
                        true,
                    ) {
                        show_window_front(&window.window);
                        windows.insert(window.window.id(), window);
                    }
                }
                Event::UserEvent(UserEvent::NewWindow(url, size)) => {
                    if let Ok(window) = build_window(
                        event_loop,
                        &mut web_context,
                        proxy.clone(),
                        next_window_label(),
                        url,
                        size,
                        true,
                    ) {
                        show_window_front(&window.window);
                        windows.insert(window.window.id(), window);
                    }
                }
                Event::UserEvent(UserEvent::NewTitle(window_id, title)) => {
                    if let Some(app_window) = windows.get(&window_id) {
                        app_window.window.set_title(&title);
                    }
                }
                #[cfg(feature = "use-favicon")]
                Event::UserEvent(UserEvent::Favicon(window_id, icon, path)) => {
                    if let Some(app_window) = windows.get(&window_id) {
                        app_window.window.set_window_icon(Some(icon));
                        set_window_favicon_path(window_id, path);
                    }
                }
                #[cfg(feature = "tray-icon")]
                Event::UserEvent(UserEvent::TrayIconEvent(event)) => {
                    if matches!(
                        event,
                        TrayIconEvent::Click {
                            button: MouseButton::Left,
                            button_state: MouseButtonState::Up,
                            ..
                        } | TrayIconEvent::DoubleClick {
                            button: MouseButton::Left,
                            ..
                        }
                    ) {
                        activate_existing_windows_or_create(
                            &mut windows,
                            event_loop,
                            &mut web_context,
                            proxy.clone(),
                        );
                    }
                }
                #[cfg(feature = "tray-icon")]
                Event::UserEvent(UserEvent::MenuEvent(event)) => {
                    if let Some(tray) = &tray {
                        let id = event.id.0.as_str();
                        if tray.new_window_id.as_deref() == Some(id) {
                            if let Ok(window) = build_window(
                                event_loop,
                                &mut web_context,
                                proxy.clone(),
                                next_window_label(),
                                selected_app_url().to_string(),
                                None,
                                true,
                            ) {
                                windows.insert(window.window.id(), window);
                            }
                        } else if tray.quit_id.as_deref() == Some(id) {
                            *control_flow = ControlFlow::Exit;
                        }
                    }
                }
                _ => {}
            }
        });
    }
}

#[cfg(target_os = "windows")]
fn main() {
    windows_app::main();
}

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!("webview-app is currently Windows-only");
}
