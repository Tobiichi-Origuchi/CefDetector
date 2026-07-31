use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::mem::size_of;
use std::os::windows::ffi::{OsStrExt as _, OsStringExt as _};
use std::path::PathBuf;
use std::ptr;

use windows_sys::Win32::Foundation::{
    ERROR_CLASS_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, WPARAM,
};
use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    ChangeWindowMessageFilterEx, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    FindWindowW, GWLP_USERDATA, GetMessageW, GetWindowLongPtrW, KillTimer, MSG, MSGFLT_ALLOW,
    PostMessageW, RegisterClassExW, SMTO_ABORTIFHUNG, SendMessageTimeoutW, SetTimer,
    SetWindowLongPtrW, TranslateMessage, WM_APP, WM_COPYDATA, WM_TIMER, WNDCLASSEXW,
};

use super::everything_protocol::{ITEM_FLAG_FOLDER, encode_query, parse_reply};
use super::{CandidateSource, ScanCandidate, classify_candidate_name};

const EVERYTHING_WINDOW_CLASS: &str = "EVERYTHING_TASKBAR_NOTIFICATION";
const REPLY_WINDOW_CLASS: &str = "CEFDETECTOR_EVERYTHING_IPC";
const EVERYTHING_COPYDATA_QUERY_W: usize = 2;
const QUERY_REPLY_ID: usize = 0x4345_4644;
const SEND_TIMEOUT_MS: u32 = 5_000;
const REPLY_TIMEOUT_MS: u32 = 30_000;
const REPLY_TIMER_ID: usize = 1;
const REPLY_RECEIVED_MESSAGE: u32 = WM_APP + 1;
const MAX_REPLY_BYTES: usize = 128 * 1024 * 1024;
const SEARCH: &str = r#"file: <_100_|libcef|libnode|"Chromium Embedded Framework">"#;

#[derive(Default)]
pub(super) struct EverythingCandidateSource;

#[derive(Default)]
struct QueryState {
    response: Option<io::Result<Vec<u8>>>,
}

struct WindowTimer {
    window: HWND,
    id: usize,
}

impl Drop for WindowTimer {
    fn drop(&mut self) {
        // SAFETY: The timer belongs to this live window and thread. It is fine
        // if the timer has already expired but its message has not been handled.
        unsafe {
            KillTimer(self.window, self.id);
        }
    }
}

struct Window(HWND);

impl Drop for Window {
    fn drop(&mut self) {
        // SAFETY: The handle was created by CreateWindowExW on this thread.
        unsafe {
            SetWindowLongPtrW(self.0, GWLP_USERDATA, 0);
            DestroyWindow(self.0);
        }
    }
}

fn wide_null(text: &str) -> Vec<u16> {
    OsStr::new(text)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

unsafe extern "system" fn reply_window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_COPYDATA {
        // SAFETY: The pointer was installed immediately after window creation
        // and is cleared before the QueryState leaves scope.
        let state = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) as *mut QueryState };
        if !state.is_null() && lparam != 0 {
            // SAFETY: Windows validates COPYDATASTRUCT for WM_COPYDATA and keeps
            // its data buffer valid for the duration of this callback.
            let copy_data = unsafe { &*(lparam as *const COPYDATASTRUCT) };
            if copy_data.dwData == QUERY_REPLY_ID {
                let result = if copy_data.cbData as usize > MAX_REPLY_BYTES {
                    Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Everything IPC reply exceeds the safety limit",
                    ))
                } else if copy_data.cbData == 0 {
                    Ok(Vec::new())
                } else if copy_data.lpData.is_null() {
                    Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Everything IPC returned a null reply buffer",
                    ))
                } else {
                    // SAFETY: lpData points to cbData bytes for this callback.
                    let bytes = unsafe {
                        std::slice::from_raw_parts(
                            copy_data.lpData.cast::<u8>(),
                            copy_data.cbData as usize,
                        )
                    };
                    Ok(bytes.to_vec())
                };
                // SAFETY: state is non-null and exclusively owned by this
                // window's thread while messages are dispatched.
                unsafe {
                    (*state).response = Some(result);
                }
                // GetMessageW dispatches this sent WM_COPYDATA internally but
                // does not return until a queued message is available. Posting
                // a private wake-up message lets the query observe the reply
                // immediately instead of waiting for the timeout timer.
                unsafe {
                    PostMessageW(window, REPLY_RECEIVED_MESSAGE, 0, 0);
                }
                return 1;
            }
        }
    }

    // SAFETY: Forwarding unhandled messages to the system default procedure is
    // required by the window contract.
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

fn register_reply_window_class(class_name: &[u16]) -> io::Result<()> {
    // SAFETY: A null module name requests the current executable module.
    let instance = unsafe { GetModuleHandleW(ptr::null()) };
    if instance.is_null() {
        return Err(io::Error::last_os_error());
    }

    let class = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(reply_window_proc),
        hInstance: instance,
        lpszClassName: class_name.as_ptr(),
        ..Default::default()
    };
    // SAFETY: class references live UTF-16 storage and a valid callback.
    if unsafe { RegisterClassExW(&class) } == 0 {
        // SAFETY: GetLastError reads the calling thread's last error value.
        let error = unsafe { GetLastError() };
        if error != ERROR_CLASS_ALREADY_EXISTS {
            return Err(io::Error::from_raw_os_error(error as i32));
        }
    }
    Ok(())
}

fn create_reply_window(state: &mut QueryState) -> io::Result<Window> {
    let class_name = wide_null(REPLY_WINDOW_CLASS);
    register_reply_window_class(&class_name)?;

    // SAFETY: A null module name requests the current executable module.
    let instance = unsafe { GetModuleHandleW(ptr::null()) };
    // SAFETY: All string pointers are null-terminated or null, and the class was
    // registered for this module.
    let window = unsafe {
        CreateWindowExW(
            0,
            class_name.as_ptr(),
            ptr::null(),
            0,
            0,
            0,
            0,
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            instance,
            ptr::null(),
        )
    };
    if window.is_null() {
        return Err(io::Error::last_os_error());
    }
    let window = Window(window);

    // SAFETY: state outlives the window and message loop.
    unsafe {
        SetWindowLongPtrW(window.0, GWLP_USERDATA, state as *mut QueryState as isize);
    }
    // Allow replies when Everything is running at a higher integrity level.
    // SAFETY: window is a live handle owned by this thread.
    if unsafe { ChangeWindowMessageFilterEx(window.0, WM_COPYDATA, MSGFLT_ALLOW, ptr::null_mut()) }
        == 0
    {
        return Err(io::Error::last_os_error());
    }

    Ok(window)
}

fn send_query(everything_window: HWND, reply_window: HWND) -> io::Result<()> {
    let reply_handle = u32::try_from(reply_window as usize)
        .map_err(|_| io::Error::other("Everything IPC reply window does not fit in 32 bits"))?;
    let search: Vec<u16> = OsStr::new(SEARCH).encode_wide().collect();
    let mut query = encode_query(reply_handle, QUERY_REPLY_ID as u32, &search)?;
    let mut copy_data = COPYDATASTRUCT {
        dwData: EVERYTHING_COPYDATA_QUERY_W,
        cbData: query
            .len()
            .try_into()
            .map_err(|_| io::Error::other("Everything query is too large"))?,
        lpData: query.as_mut_ptr().cast(),
    };
    let mut message_result = 0;

    // SAFETY: copy_data and query remain valid until SendMessageTimeoutW
    // returns. Everything copies the query before returning.
    let sent = unsafe {
        SendMessageTimeoutW(
            everything_window,
            WM_COPYDATA,
            reply_window as usize,
            &mut copy_data as *mut COPYDATASTRUCT as isize,
            SMTO_ABORTIFHUNG,
            SEND_TIMEOUT_MS,
            &mut message_result,
        )
    };
    if sent == 0 {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!(
                "Everything did not accept the IPC query: {}",
                io::Error::last_os_error()
            ),
        ));
    }
    if message_result == 0 {
        return Err(io::Error::other(
            "Everything rejected the IPC query; make sure IPC is enabled",
        ));
    }
    Ok(())
}

fn run_ipc_query() -> io::Result<Vec<u8>> {
    let everything_class = wide_null(EVERYTHING_WINDOW_CLASS);
    // SAFETY: everything_class is null-terminated and the title is unspecified.
    let everything_window = unsafe { FindWindowW(everything_class.as_ptr(), ptr::null()) };
    if everything_window.is_null() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "Everything is not running; start the Everything desktop client and enable IPC",
        ));
    }

    let mut state = QueryState::default();
    let reply_window = create_reply_window(&mut state)?;
    send_query(everything_window, reply_window.0)?;
    if let Some(response) = state.response.take() {
        return response;
    }

    // A window timer wakes GetMessageW even if Everything accepts the query and
    // then exits or otherwise fails to send a reply. Keeping the timeout inside
    // this thread avoids leaking a permanently blocked detached worker.
    // SAFETY: reply_window is live and owned by the current thread.
    let timer_id = unsafe { SetTimer(reply_window.0, REPLY_TIMER_ID, REPLY_TIMEOUT_MS, None) };
    if timer_id == 0 {
        return Err(io::Error::last_os_error());
    }
    let _timer = WindowTimer {
        window: reply_window.0,
        id: timer_id,
    };
    if let Some(response) = state.response.take() {
        return response;
    }

    loop {
        let mut message = MSG::default();
        // SAFETY: message points to writable storage and the current thread owns
        // the reply window and its message queue.
        let status = unsafe { GetMessageW(&mut message, ptr::null_mut(), 0, 0) };
        if status == -1 {
            return Err(io::Error::last_os_error());
        }
        if status == 0 {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Everything IPC message loop ended before a reply arrived",
            ));
        }
        // GetMessageW dispatches cross-thread sent messages before returning a
        // queued message. A WM_COPYDATA reply can therefore already be stored
        // even when the returned message is the timeout timer.
        if let Some(response) = state.response.take() {
            return response;
        }
        if message.hwnd == reply_window.0
            && message.message == WM_TIMER
            && message.wParam == timer_id
        {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Everything did not return search results within 30 seconds",
            ));
        }
        // SAFETY: message was initialized by GetMessageW.
        unsafe {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        if let Some(response) = state.response.take() {
            return response;
        }
    }
}

impl CandidateSource for EverythingCandidateSource {
    fn find_candidates(&self) -> io::Result<Vec<ScanCandidate>> {
        let reply = run_ipc_query()?;
        let mut seen = HashSet::new();
        let mut candidates = Vec::new();

        for item in parse_reply(&reply)? {
            if item.flags & ITEM_FLAG_FOLDER != 0 {
                continue;
            }
            let file_name = OsString::from_wide(&item.file_name);
            let Some(kind) = classify_candidate_name(file_name.to_string_lossy().as_ref()) else {
                continue;
            };
            let mut path = PathBuf::from(OsString::from_wide(&item.path));
            path.push(file_name);
            if !seen.insert(path.clone()) {
                continue;
            }
            if !fs::metadata(&path).is_ok_and(|metadata| metadata.is_file()) {
                continue;
            }
            candidates.push(ScanCandidate {
                path,
                kind,
                #[cfg(target_os = "macos")]
                application_root_hint: None,
            });
        }

        candidates.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(candidates)
    }
}
