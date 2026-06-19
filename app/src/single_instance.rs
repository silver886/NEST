use std::thread;

use windows_sys::Win32::{
    Foundation::{
        CloseHandle, ERROR_ALREADY_EXISTS, ERROR_PIPE_CONNECTED, GetLastError, INVALID_HANDLE_VALUE,
    },
    Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
        PIPE_ACCESS_INBOUND, ReadFile, WriteFile,
    },
    System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_MESSAGE,
        PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_MESSAGE, PIPE_WAIT, WaitNamedPipeW,
    },
    System::Threading::{CreateMutexW, ReleaseMutex},
};

pub struct InstanceLock(windows_sys::Win32::Foundation::HANDLE);

impl Drop for InstanceLock {
    fn drop(&mut self) {
        unsafe {
            ReleaseMutex(self.0);
            CloseHandle(self.0);
        }
    }
}

pub enum InstanceClaim {
    Disabled,
    Primary(InstanceLock),
    Secondary,
    Failed,
}

fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn safe_app_id(app_id: &str) -> String {
    app_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

fn pipe_name(app_id: &str) -> Vec<u16> {
    wide_null(&format!(r"\\.\pipe\nest-{}", safe_app_id(app_id)))
}

fn mutex_name(app_id: &str) -> Vec<u16> {
    wide_null(&format!("Local\\nest-{}-instance", safe_app_id(app_id)))
}

pub fn claim_primary(app_id: &str, enabled: bool) -> InstanceClaim {
    if !enabled {
        return InstanceClaim::Disabled;
    }

    let name = mutex_name(app_id);
    let mutex = unsafe { CreateMutexW(std::ptr::null(), 1, name.as_ptr()) };
    if mutex.is_null() {
        return InstanceClaim::Failed;
    }

    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe {
            CloseHandle(mutex);
        }
        InstanceClaim::Secondary
    } else {
        InstanceClaim::Primary(InstanceLock(mutex))
    }
}

pub fn send_to_primary(app_id: &str, arg: Option<&str>) -> bool {
    let name = pipe_name(app_id);
    let payload = arg.unwrap_or_default().as_bytes();

    if unsafe { WaitNamedPipeW(name.as_ptr(), 2_000) } == 0 {
        return false;
    }

    let pipe = unsafe {
        CreateFileW(
            name.as_ptr(),
            windows_sys::Win32::Foundation::GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        )
    };
    if pipe == INVALID_HANDLE_VALUE {
        return false;
    }

    let mut bytes_written = 0;
    let ok = unsafe {
        WriteFile(
            pipe,
            payload.as_ptr(),
            payload.len().try_into().unwrap_or(u32::MAX),
            &mut bytes_written,
            std::ptr::null_mut(),
        ) != 0
    };

    unsafe {
        CloseHandle(pipe);
    }

    ok
}

pub fn listen(app_id: &str, mut on_message: impl FnMut(String) + Send + 'static) {
    let name = pipe_name(app_id);

    thread::spawn(move || {
        loop {
            let pipe = unsafe {
                CreateNamedPipeW(
                    name.as_ptr(),
                    PIPE_ACCESS_INBOUND,
                    PIPE_TYPE_MESSAGE
                        | PIPE_READMODE_MESSAGE
                        | PIPE_WAIT
                        | PIPE_REJECT_REMOTE_CLIENTS,
                    1,
                    4096,
                    4096,
                    500,
                    std::ptr::null(),
                )
            };
            if pipe == INVALID_HANDLE_VALUE {
                return;
            }

            let connected = unsafe { ConnectNamedPipe(pipe, std::ptr::null_mut()) != 0 }
                || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;

            if connected {
                let mut buffer = [0u8; 4096];
                let mut bytes_read = 0;
                let ok = unsafe {
                    ReadFile(
                        pipe,
                        buffer.as_mut_ptr(),
                        buffer.len() as u32,
                        &mut bytes_read,
                        std::ptr::null_mut(),
                    ) != 0
                };

                if ok {
                    let message = String::from_utf8_lossy(&buffer[..bytes_read as usize])
                        .trim_end_matches(&['\r', '\n'][..])
                        .to_string();
                    on_message(message);
                }
            }

            unsafe {
                DisconnectNamedPipe(pipe);
                CloseHandle(pipe);
            }
        }
    });
}
