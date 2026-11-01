//! The few things that differ by operating system: the list of screens,
//! "is the user in a full-screen app?", "is the microphone in use?", the
//! local time, start-at-login, and telling an already-running Bit to open
//! its settings.

use crate::flight::Monitor;

#[cfg(windows)]
pub use win::*;

#[cfg(not(windows))]
pub use other::*;

#[cfg(windows)]
mod win {
    use super::Monitor;
    use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
    use windows_sys::Win32::{
        Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
        Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO},
        System::LibraryLoader::GetModuleHandleW,
        System::Registry::{
            RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ,
            RRF_RT_REG_SZ,
        },
        UI::HiDpi::{
            GetDpiForMonitor, SetProcessDpiAwarenessContext,
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, MDT_EFFECTIVE_DPI,
        },
        UI::Shell::{
            SHQueryUserNotificationState, QUNS_BUSY, QUNS_PRESENTATION_MODE,
            QUNS_RUNNING_D3D_FULL_SCREEN,
        },
        UI::WindowsAndMessaging::{
            AllowSetForegroundWindow, CreateWindowExW, DefWindowProcW, FindWindowW, GetCursorPos,
            PostMessageW,
            RegisterClassW, SetForegroundWindow, ShowWindow, ASFW_ANY, WM_APP, WNDCLASSW,
            WS_EX_TOOLWINDOW, WS_POPUP,
        },
    };

    pub(crate) fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    /// Work in real pixels on every screen (no blurry bitmap stretching).
    pub fn init() {
        unsafe {
            SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }
    }

    pub fn monitors() -> Vec<Monitor> {
        unsafe extern "system" fn each(
            m: HMONITOR,
            _: HDC,
            _: *mut RECT,
            data: LPARAM,
        ) -> windows_sys::core::BOOL {
            let list = &mut *(data as *mut Vec<Monitor>);
            let mut info = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if GetMonitorInfoW(m, &mut info) != 0 {
                let (mut dx, mut dy) = (96, 96);
                GetDpiForMonitor(m, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
                let r = info.rcMonitor;
                list.push(Monitor {
                    x: r.left as f32,
                    y: r.top as f32,
                    w: (r.right - r.left) as f32,
                    h: (r.bottom - r.top) as f32,
                    scale: dx as f32 / 96.0,
                    primary: info.dwFlags & 1 != 0,
                });
            }
            1
        }
        let mut list = Vec::new();
        unsafe {
            EnumDisplayMonitors(
                std::ptr::null_mut(),
                std::ptr::null(),
                Some(each),
                &mut list as *mut _ as LPARAM,
            );
        }
        if list.is_empty() {
            list.push(Monitor {
                x: 0.0,
                y: 0.0,
                w: 1920.0,
                h: 1080.0,
                scale: 1.0,
                primary: true,
            });
        }
        list
    }

    /// A full-screen game, a presentation, or "do not disturb": Bit stays away.
    pub fn busy() -> bool {
        let mut state = 0;
        let ok = unsafe { SHQueryUserNotificationState(&mut state) } == 0;
        ok && matches!(
            state,
            QUNS_BUSY | QUNS_RUNNING_D3D_FULL_SCREEN | QUNS_PRESENTATION_MODE
        )
    }

    /// Some app is using the microphone right now (a call or a recording).
    /// Windows logs microphone use per app; an entry that has started but
    /// not stopped is in use.
    pub fn mic_in_use() -> bool {
        use windows_sys::Win32::System::Registry::{
            RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, HKEY, KEY_READ, RRF_RT_REG_QWORD,
        };
        /// A QWORD value of `key`, or `None` if it isn't there.
        unsafe fn qword(key: HKEY, name: &str) -> Option<u64> {
            let (name, mut v, mut size) = (wide(name), 0u64, 8u32);
            let ok = RegGetValueW(
                key,
                std::ptr::null(),
                name.as_ptr(),
                RRF_RT_REG_QWORD,
                std::ptr::null_mut(),
                (&mut v as *mut u64).cast(),
                &mut size,
            );
            (ok == 0).then_some(v)
        }
        /// Any app under `key` in use (looking one level into "NonPackaged",
        /// where ordinary desktop programs are listed).
        unsafe fn any_in_use(key: HKEY, top: bool) -> bool {
            for i in 0.. {
                let mut name = [0u16; 512];
                let mut len = name.len() as u32;
                let found = RegEnumKeyExW(
                    key,
                    i,
                    name.as_mut_ptr(),
                    &mut len,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                );
                if found != 0 {
                    break;
                }
                let mut sub: HKEY = std::ptr::null_mut();
                if RegOpenKeyExW(key, name.as_ptr(), 0, KEY_READ, &mut sub) != 0 {
                    continue;
                }
                let started = qword(sub, "LastUsedTimeStart").is_some_and(|t| t != 0);
                let busy = (started && qword(sub, "LastUsedTimeStop") == Some(0))
                    || (top
                        && String::from_utf16_lossy(&name[..len as usize]) == "NonPackaged"
                        && any_in_use(sub, false));
                RegCloseKey(sub);
                if busy {
                    return true;
                }
            }
            false
        }
        let path = wide(
            "Software\\Microsoft\\Windows\\CurrentVersion\\CapabilityAccessManager\\ConsentStore\\microphone",
        );
        unsafe {
            let mut key: HKEY = std::ptr::null_mut();
            if RegOpenKeyExW(HKEY_CURRENT_USER, path.as_ptr(), 0, KEY_READ, &mut key) != 0 {
                return false;
            }
            let busy = any_in_use(key, true);
            RegCloseKey(key);
            busy
        }
    }

    /// Minutes since local midnight.
    pub fn local_minutes() -> u32 {
        let mut t = unsafe { std::mem::zeroed() };
        unsafe { windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut t) };
        let t: windows_sys::Win32::Foundation::SYSTEMTIME = t;
        u32::from(t.wHour) * 60 + u32::from(t.wMinute)
    }

    const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
    const RUN_VALUE: &str = "Tron Bit";

    pub fn autostart_supported() -> bool {
        true
    }

    pub fn autostart() -> bool {
        let (key, value) = (wide(RUN_KEY), wide(RUN_VALUE));
        unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            ) == 0
        }
    }

    pub fn set_autostart(on: bool) {
        let (key, value) = (wide(RUN_KEY), wide(RUN_VALUE));
        unsafe {
            if on {
                let Ok(exe) = std::env::current_exe() else {
                    return;
                };
                let cmd = wide(&format!("\"{}\"", exe.display()));
                RegSetKeyValueW(
                    HKEY_CURRENT_USER,
                    key.as_ptr(),
                    value.as_ptr(),
                    REG_SZ,
                    cmd.as_ptr().cast(),
                    (cmd.len() * 2) as u32,
                );
            } else {
                RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr());
            }
        }
    }

    const INSTANCE_CLASS: &str = "TronBitDesktopInstance";
    const SHOW_SETTINGS: u32 = WM_APP + 7;
    const SETTINGS_CHANGED: u32 = WM_APP + 8;
    const QUIT_BIT: u32 = WM_APP + 9;
    const WM_HOTKEY: u32 = 0x0312;
    static ON_SIGNAL: AtomicIsize = AtomicIsize::new(0);
    static ON_HOTKEY: AtomicIsize = AtomicIsize::new(0);
    static ON_CHANGED: AtomicIsize = AtomicIsize::new(0);
    static ON_QUIT: AtomicIsize = AtomicIsize::new(0);

    /// Let a second copy of Bit, started later, ask this one to open its
    /// settings, listen for the Ctrl+Alt+B "ask Bit" hotkey, and run
    /// `on_changed` / `on_quit` when another thread asks
    /// (`tell_settings_changed`, `ask_main_to_quit`). Call on the main (UI)
    /// thread. Returns whether the hotkey could be had.
    pub fn listen(
        key: &str,
        on_signal: fn(),
        on_hotkey: fn(),
        on_changed: fn(),
        on_quit: fn(),
    ) -> bool {
        unsafe extern "system" fn proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
            let call = |f: &AtomicIsize| {
                let f = f.load(Ordering::Relaxed);
                if f != 0 {
                    std::mem::transmute::<isize, fn()>(f)();
                }
            };
            match msg {
                SHOW_SETTINGS => call(&ON_SIGNAL),
                WM_HOTKEY => call(&ON_HOTKEY),
                SETTINGS_CHANGED => call(&ON_CHANGED),
                QUIT_BIT => call(&ON_QUIT),
                _ => return DefWindowProcW(hwnd, msg, w, l),
            }
            0
        }
        ON_SIGNAL.store(on_signal as isize, Ordering::Relaxed);
        ON_HOTKEY.store(on_hotkey as isize, Ordering::Relaxed);
        ON_CHANGED.store(on_changed as isize, Ordering::Relaxed);
        ON_QUIT.store(on_quit as isize, Ordering::Relaxed);
        let class = wide(INSTANCE_CLASS);
        unsafe {
            let instance = GetModuleHandleW(std::ptr::null());
            let wc = WNDCLASSW {
                lpfnWndProc: Some(proc),
                hInstance: instance,
                lpszClassName: class.as_ptr(),
                ..Default::default()
            };
            RegisterClassW(&wc);
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                class.as_ptr(),
                class.as_ptr(),
                WS_POPUP,
                0,
                0,
                0,
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                instance,
                std::ptr::null(),
            );
            HOTKEY_WINDOW.store(hwnd as isize, Ordering::Relaxed);
        }
        set_hotkey(key)
    }

    static HOTKEY_WINDOW: AtomicIsize = AtomicIsize::new(0);

    /// Make `key` ("Ctrl+Alt+B", or empty for none) the Ask Bit key, in place
    /// of any before it. Call on the main (UI) thread. False if another
    /// program has it, or there isn't one.
    pub fn set_hotkey(key: &str) -> bool {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
            RegisterHotKey, UnregisterHotKey, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT,
        };
        let hwnd = HOTKEY_WINDOW.load(Ordering::Relaxed) as HWND;
        if hwnd.is_null() {
            return false;
        }
        unsafe {
            UnregisterHotKey(hwnd, 1);
        }
        let Some((ctrl, alt, shift, code)) = crate::settings::hotkey_parts(key) else {
            return false;
        };
        let mut mods = MOD_NOREPEAT;
        for (on, m) in [(ctrl, MOD_CONTROL), (alt, MOD_ALT), (shift, MOD_SHIFT)] {
            if on {
                mods |= m;
            }
        }
        unsafe { RegisterHotKey(hwnd, 1, mods, code) != 0 }
    }

    /// We are the second copy: ask the first to open its settings.
    pub fn signal_first_instance() {
        let class = wide(INSTANCE_CLASS);
        unsafe {
            let hwnd = FindWindowW(class.as_ptr(), std::ptr::null());
            if !hwnd.is_null() {
                AllowSetForegroundWindow(ASFW_ANY);
                PostMessageW(hwnd, SHOW_SETTINGS, 0, 0);
            }
        }
    }

    /// From any thread: have the main thread run `listen`'s `on_changed`.
    pub fn tell_settings_changed() {
        post_main(SETTINGS_CHANGED);
    }

    /// From any thread: have the main thread run `listen`'s `on_quit`.
    pub fn ask_main_to_quit() {
        post_main(QUIT_BIT);
    }

    fn post_main(msg: u32) {
        let class = wide(INSTANCE_CLASS);
        unsafe {
            // (Our own window: the settings process never makes one.)
            let hwnd = FindWindowW(class.as_ptr(), std::ptr::null());
            if !hwnd.is_null() {
                PostMessageW(hwnd, msg, 0, 0);
            }
        }
    }

    /// The keys held right now: the modifiers, written "Ctrl+Alt+", and any
    /// one key that could finish a combination ("B", "F9", "Esc", "Delete").
    /// Read directly, because a window never sees a combination another
    /// program has claimed, and Windows treats Ctrl+Alt as AltGr.
    pub fn read_keys() -> (String, Option<String>) {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
            keybd_event, GetAsyncKeyState, KEYEVENTF_KEYUP,
        };
        let down = |vk: i32| unsafe { GetAsyncKeyState(vk) } < 0;
        // A key event between Alt going down and up stops Alt alone from
        // opening the window's menu, which would swallow the next click.
        if down(0x12) && !ALT_MASKED.swap(true, Ordering::Relaxed) {
            unsafe {
                keybd_event(0xE8, 0, 0, 0);
                keybd_event(0xE8, 0, KEYEVENTF_KEYUP, 0);
            }
        } else if !down(0x12) {
            ALT_MASKED.store(false, Ordering::Relaxed);
        }
        let mut mods = String::new();
        for (vk, name) in [(0x11, "Ctrl+"), (0x12, "Alt+"), (0x10, "Shift+")] {
            if down(vk) {
                mods.push_str(name);
            }
        }
        let key = (0x30..=0x39)
            .chain(0x41..=0x5A)
            .find(|&vk| down(vk))
            .map(|vk| (vk as u8 as char).to_string())
            .or_else(|| (0x70..=0x87).find(|&vk| down(vk)).map(|vk| format!("F{}", vk - 0x6F)))
            .or_else(|| down(0x1B).then(|| "Esc".to_string()))
            .or_else(|| (down(0x2E) || down(0x08)).then(|| "Delete".to_string()));
        (mods, key)
    }

    static ALT_MASKED: AtomicBool = AtomicBool::new(false);

    /// Done reading keys: if an Alt combination left the window waiting in
    /// its menu mode, end that, so the next click reaches it.
    pub fn end_keys() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            EndMenu, GetForegroundWindow, SendMessageW, WM_CANCELMODE,
        };
        unsafe {
            EndMenu();
            SendMessageW(GetForegroundWindow(), WM_CANCELMODE, 0, 0);
        }
    }

    /// Whether the window in front belongs to this process.
    pub fn in_front() -> bool {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetForegroundWindow, GetWindowThreadProcessId,
        };
        let mut owner = 0;
        unsafe {
            GetWindowThreadProcessId(GetForegroundWindow(), &mut owner);
        }
        owner == std::process::id()
    }

    /// Where the pointer is, in desktop pixels.
    pub fn cursor() -> [f32; 2] {
        let mut p = POINT { x: 0, y: 0 };
        unsafe {
            GetCursorPos(&mut p);
        }
        [p.x as f32, p.y as f32]
    }

    /// Let process `pid` bring its window to the front.
    pub fn allow_foreground(pid: u32) {
        unsafe {
            AllowSetForegroundWindow(pid);
        }
    }

    /// Bring process `pid`'s window to the front (restored if minimized).
    pub fn raise_window_of(pid: u32) {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            EnumWindows, GetWindowThreadProcessId, IsIconic, IsWindowVisible, SW_RESTORE,
        };
        unsafe extern "system" fn each(hwnd: HWND, data: LPARAM) -> windows_sys::core::BOOL {
            let (pid, found) = &mut *(data as *mut (u32, HWND));
            let mut owner = 0;
            GetWindowThreadProcessId(hwnd, &mut owner);
            if owner == *pid && IsWindowVisible(hwnd) != 0 {
                *found = hwnd;
                return 0;
            }
            1
        }
        let mut data: (u32, HWND) = (pid, std::ptr::null_mut());
        unsafe {
            EnumWindows(Some(each), &mut data as *mut _ as LPARAM);
            let hwnd = data.1;
            if !hwnd.is_null() {
                if IsIconic(hwnd) != 0 {
                    ShowWindow(hwnd, SW_RESTORE);
                }
                SetForegroundWindow(hwnd);
            }
        }
    }

    /// The longest gap between the two clicks of a double-click (s).
    pub fn double_click_seconds() -> f32 {
        let ms = unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime() };
        (ms as f32 / 1000.0).clamp(0.2, 0.9)
    }

    /// Handle this thread's window messages (the tray's) until `flag` is set.
    pub fn wait_until(flag: &std::sync::atomic::AtomicBool) {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            DispatchMessageW, GetMessageW, TranslateMessage, MSG,
        };
        unsafe {
            let mut msg: MSG = std::mem::zeroed();
            while !flag.load(Ordering::Relaxed) {
                if GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) <= 0 {
                    crate::quit();
                }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

#[cfg(not(windows))]
mod other {
    use super::Monitor;
    use std::sync::Mutex;

    /// The screen size, as last seen by the settings window.
    pub static SCREEN: Mutex<Option<(f32, f32, f32)>> = Mutex::new(None);

    pub fn init() {}

    pub fn monitors() -> Vec<Monitor> {
        let (w, h, scale) = SCREEN
            .lock()
            .ok()
            .and_then(|s| *s)
            .unwrap_or((1920.0, 1080.0, 1.0));
        vec![Monitor {
            x: 0.0,
            y: 0.0,
            w,
            h,
            scale,
            primary: true,
        }]
    }

    pub fn busy() -> bool {
        false
    }

    pub fn mic_in_use() -> bool {
        false
    }

    /// Minutes since local midnight.
    pub fn local_minutes() -> u32 {
        // SAFETY: plain C calls; `localtime_r` fills the struct we own.
        unsafe {
            let now = libc::time(std::ptr::null_mut());
            let mut tm: libc::tm = std::mem::zeroed();
            if libc::localtime_r(&now, &mut tm).is_null() {
                return 12 * 60;
            }
            (tm.tm_hour * 60 + tm.tm_min) as u32
        }
    }

    pub fn autostart_supported() -> bool {
        false
    }

    pub fn autostart() -> bool {
        false
    }

    pub fn set_autostart(_on: bool) {}

    pub fn listen(
        _key: &str,
        _on_signal: fn(),
        _on_hotkey: fn(),
        _on_changed: fn(),
        _on_quit: fn(),
    ) -> bool {
        false
    }

    pub fn set_hotkey(_key: &str) -> bool {
        false
    }

    pub fn cursor() -> [f32; 2] {
        [0.0, 0.0]
    }

    pub fn read_keys() -> (String, Option<String>) {
        (String::new(), None)
    }

    pub fn end_keys() {}

    pub fn in_front() -> bool {
        true
    }

    pub fn tell_settings_changed() {}

    pub fn double_click_seconds() -> f32 {
        0.5
    }

    pub fn signal_first_instance() {}

}
