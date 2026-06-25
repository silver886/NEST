use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::Once,
};

use windows::{
    Data::Xml::Dom::XmlDocument,
    UI::Notifications::{ToastNotification, ToastNotificationManager},
    Win32::{
        Foundation::PROPERTYKEY,
        System::{
            Com::StructuredStorage::{
                PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0, PropVariantClear,
            },
            Com::{
                CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
                IPersistFile,
            },
            Variant::VT_LPWSTR,
        },
        UI::Shell::{
            IShellLinkW, PropertiesSystem::IPropertyStore, SHStrDupW,
            SetCurrentProcessExplicitAppUserModelID, ShellLink,
        },
    },
    core::{HSTRING, Interface, Result},
};

static INIT_COM: Once = Once::new();

const PKEY_APP_USER_MODEL_ID: PROPERTYKEY = PROPERTYKEY {
    fmtid: windows::core::GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3),
    pid: 5,
};

pub fn install(app_id: &str, title: &str) {
    let _ = init(app_id, title);
}

pub fn notify(
    app_id: &str,
    title: &str,
    body: &str,
    download_path: Option<&Path>,
    icon_path: Option<&Path>,
) {
    let _ = show_toast(app_id, title, body, download_path, icon_path);
}

fn init(app_id: &str, title: &str) -> Result<()> {
    init_com();
    let app_id = HSTRING::from(app_id);
    unsafe {
        SetCurrentProcessExplicitAppUserModelID(&app_id)?;
    }
    ensure_start_menu_shortcut(&app_id, title)
}

fn init_com() {
    INIT_COM.call_once(|| unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    });
}

fn ensure_start_menu_shortcut(app_id: &HSTRING, title: &str) -> Result<()> {
    init_com();

    let Some(exe) = env::current_exe().ok() else {
        return Ok(());
    };
    let shortcut = shortcut_path(title);
    if let Some(parent) = shortcut.parent() {
        let _ = fs::create_dir_all(parent);
    }

    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        link.SetPath(&HSTRING::from(exe.as_os_str().to_string_lossy().as_ref()))?;
        link.SetIconLocation(
            &HSTRING::from(exe.as_os_str().to_string_lossy().as_ref()),
            0,
        )?;
        link.SetWorkingDirectory(&HSTRING::from(
            exe.parent()
                .unwrap_or_else(|| std::path::Path::new(""))
                .as_os_str()
                .to_string_lossy()
                .as_ref(),
        ))?;
        link.SetDescription(&HSTRING::from(title))?;

        let store: IPropertyStore = link.cast()?;
        let mut value = propvariant_from_string(app_id)?;
        store.SetValue(&PKEY_APP_USER_MODEL_ID, &value)?;
        store.Commit()?;
        let _ = PropVariantClear(&mut value as *mut PROPVARIANT);

        let file: IPersistFile = link.cast()?;
        file.Save(
            &HSTRING::from(shortcut.as_os_str().to_string_lossy().as_ref()),
            true,
        )?;
    }

    Ok(())
}

unsafe fn propvariant_from_string(value: &HSTRING) -> Result<PROPVARIANT> {
    let pwsz = unsafe { SHStrDupW(value)? };
    Ok(PROPVARIANT {
        Anonymous: PROPVARIANT_0 {
            Anonymous: std::mem::ManuallyDrop::new(PROPVARIANT_0_0 {
                vt: VT_LPWSTR,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: PROPVARIANT_0_0_0 { pwszVal: pwsz },
            }),
        },
    })
}

fn shortcut_path(title: &str) -> PathBuf {
    let base = env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(env::temp_dir);
    base.join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs")
        .join("Nest")
        .join(format!("{}.lnk", safe_filename(title)))
}

fn safe_filename(title: &str) -> String {
    let name = title
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '-',
            c if c.is_control() => '-',
            c => c,
        })
        .collect::<String>();
    if name.trim().is_empty() {
        "Nest".to_string()
    } else {
        name
    }
}

fn show_toast(
    app_id: &str,
    title: &str,
    body: &str,
    download_path: Option<&Path>,
    icon_path: Option<&Path>,
) -> Result<()> {
    let file_uri = download_path.map(path_to_file_uri);
    let folder_uri = download_path
        .and_then(|path| path.parent())
        .map(path_to_file_uri);
    let icon = icon_path
        .map(path_to_file_uri)
        .map(|uri| {
            format!(
                r#"<image placement="appLogoOverride" src="{}"/>"#,
                escape_xml(&uri)
            )
        })
        .unwrap_or_default();
    let launch = file_uri
        .as_ref()
        .map(|uri| format!(r#" activationType="protocol" launch="{}""#, escape_xml(uri)))
        .unwrap_or_default();
    let actions = match (&file_uri, &folder_uri) {
        (Some(file_uri), Some(folder_uri)) => format!(
            r#"<actions><action content="Open file" activationType="protocol" arguments="{}"/><action content="Open folder" activationType="protocol" arguments="{}"/></actions>"#,
            escape_xml(file_uri),
            escape_xml(folder_uri)
        ),
        (Some(file_uri), None) => format!(
            r#"<actions><action content="Open file" activationType="protocol" arguments="{}"/></actions>"#,
            escape_xml(file_uri)
        ),
        _ => String::new(),
    };
    let xml = XmlDocument::new()?;
    xml.LoadXml(&HSTRING::from(format!(
        r#"<toast{}><visual><binding template="ToastGeneric">{}<text>{}</text><text>{}</text></binding></visual>{}</toast>"#,
        launch,
        icon,
        escape_xml(title),
        escape_xml(body),
        actions
    )))?;

    let notification = ToastNotification::CreateToastNotification(&xml)?;
    let notifier = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(app_id))?;
    notifier.Show(&notification)
}

fn path_to_file_uri(path: &Path) -> String {
    let path = path.as_os_str().to_string_lossy().replace('\\', "/");
    format!("file:///{}", uri_encode_path(&path))
}

fn uri_encode_path(path: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(path.len());

    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/' | b':') {
            encoded.push(byte as char);
        } else {
            encoded.push('%');
            encoded.push(HEX[(byte >> 4) as usize] as char);
            encoded.push(HEX[(byte & 0x0f) as usize] as char);
        }
    }

    encoded
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
