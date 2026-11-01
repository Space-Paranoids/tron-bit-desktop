//! The see-through window Bit flies in.
//!
//! On Windows Bit normally flies in a layered window: each frame Bit's
//! premultiplied pixels go straight to the compositor with one
//! `UpdateLayeredWindow`, which also moves and resizes it. That is the
//! leanest way, but Windows won't leave a layered window out of
//! screenshots, so with "Hide from captures" on Bit is drawn through
//! DirectComposition instead (a window with no surface of its own showing a
//! small Direct3D swap chain), which costs a little more memory and CPU.
//! Either way frames are paced to the display with `DwmFlush`, and the
//! window (and any Direct3D device) exists only during a visit.
//!
//! Elsewhere the frame is handed to the settings app, which shows it in a
//! transparent egui viewport.

#[cfg(windows)]
pub use win::Overlay;

#[cfg(not(windows))]
pub use other::{Overlay, OverlayFrame};

#[cfg(windows)]
mod composed {
    use crate::platform::wide;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Once;
    use windows::core::Interface;
    use windows::Win32::Foundation::{HMODULE, HWND as WinHwnd};
    use windows::Win32::Graphics::Direct3D::{
        D3D_DRIVER_TYPE, D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP,
    };
    use windows::Win32::Graphics::Direct3D11::{
        D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11RenderTargetView,
        ID3D11Texture2D, D3D11_BOX, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION,
    };
    use windows::Win32::Graphics::DirectComposition::{
        DCompositionCreateDevice, IDCompositionDevice, IDCompositionTarget, IDCompositionVisual,
    };
    use windows::Win32::Graphics::Dxgi::Common::{
        DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_UNKNOWN,
        DXGI_SAMPLE_DESC,
    };
    use windows::Win32::Graphics::Dxgi::{
        CreateDXGIFactory2, IDXGIDevice, IDXGIFactory2, IDXGISwapChain1, DXGI_CREATE_FACTORY_FLAGS,
        DXGI_PRESENT, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG,
        DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL, DXGI_USAGE_RENDER_TARGET_OUTPUT,
    };
    use windows_sys::Win32::{
        Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM},
        Graphics::Dwm::DwmFlush,
        System::LibraryLoader::GetModuleHandleW,
        UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture},
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetCursorPos,
            GetWindowLongPtrW, LoadCursorW, PeekMessageW, RegisterClassW, SetWindowDisplayAffinity,
            SetWindowLongPtrW, SetWindowPos, ShowWindow, TranslateMessage, GWL_EXSTYLE,
            HWND_TOPMOST, IDC_HAND, MA_NOACTIVATE, MSG, PM_REMOVE, SWP_NOACTIVATE, SWP_NOZORDER,
            SW_HIDE, SW_SHOWNOACTIVATE, WDA_EXCLUDEFROMCAPTURE, WM_CAPTURECHANGED, WM_LBUTTONDOWN,
            WM_LBUTTONUP, WM_MOUSEACTIVATE,
            WNDCLASSW, WS_EX_NOACTIVATE, WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW,
            WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
        },
    };

    const CLASS: &str = "TronBitComposedOverlay";
    static DOWN: AtomicBool = AtomicBool::new(false);
    static RELEASED: AtomicBool = AtomicBool::new(false);

    unsafe extern "system" fn proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        match msg {
            WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
            WM_LBUTTONDOWN => {
                DOWN.store(true, Ordering::Relaxed);
                SetCapture(hwnd);
                0
            }
            WM_LBUTTONUP => {
                let held = DOWN.swap(false, Ordering::Relaxed);
                RELEASED.fetch_or(held, Ordering::Relaxed);
                ReleaseCapture();
                0
            }
            WM_CAPTURECHANGED => {
                let held = DOWN.swap(false, Ordering::Relaxed);
                RELEASED.fetch_or(held, Ordering::Relaxed);
                0
            }
            _ => DefWindowProcW(hwnd, msg, w, l),
        }
    }

    /// The Direct3D and DirectComposition objects showing Bit's frames.
    struct Gpu {
        context: ID3D11DeviceContext,
        device: ID3D11Device,
        swap: IDXGISwapChain1,
        _target: IDCompositionTarget,
        _visual: IDCompositionVisual,
        _dcomp: IDCompositionDevice,
    }

    impl Gpu {
        unsafe fn new(hwnd: HWND, side: u32) -> windows::core::Result<Gpu> {
            // The graphics card if there is one, else Windows' software renderer.
            let make = |kind: D3D_DRIVER_TYPE| {
                let (mut device, mut context) = (None, None);
                D3D11CreateDevice(
                    None,
                    kind,
                    HMODULE::default(),
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    None,
                    D3D11_SDK_VERSION,
                    Some(&mut device),
                    None,
                    Some(&mut context),
                )
                .map(|()| (device.unwrap(), context.unwrap()))
            };
            let (device, context) =
                make(D3D_DRIVER_TYPE_HARDWARE).or_else(|_| make(D3D_DRIVER_TYPE_WARP))?;
            let factory: IDXGIFactory2 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))?;
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: side,
                Height: side,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
                ..Default::default()
            };
            let swap = factory.CreateSwapChainForComposition(&device, &desc, None)?;
            let dxgi: IDXGIDevice = device.cast()?;
            let dcomp: IDCompositionDevice = DCompositionCreateDevice(&dxgi)?;
            let target = dcomp.CreateTargetForHwnd(WinHwnd(hwnd), true)?;
            let visual = dcomp.CreateVisual()?;
            visual.SetContent(&swap)?;
            target.SetRoot(&visual)?;
            dcomp.Commit()?;
            Ok(Gpu {
                context,
                device,
                swap,
                _target: target,
                _visual: visual,
                _dcomp: dcomp,
            })
        }

        /// Put `pixels` (`side` x `side`) in the middle of the next frame,
        /// `off` pixels in from its top left, the rest see-through, and show it.
        unsafe fn show(&self, side: usize, off: u32, pixels: &[u32]) -> windows::core::Result<()> {
            let buffer: ID3D11Texture2D = self.swap.GetBuffer(0)?;
            let mut view: Option<ID3D11RenderTargetView> = None;
            self.device
                .CreateRenderTargetView(&buffer, None, Some(&mut view))?;
            if let Some(view) = &view {
                self.context.ClearRenderTargetView(view, &[0.0; 4]);
            }
            let area = D3D11_BOX {
                left: off,
                top: off,
                front: 0,
                right: off + side as u32,
                bottom: off + side as u32,
                back: 1,
            };
            self.context.UpdateSubresource(
                &buffer,
                0,
                Some(&area),
                pixels.as_ptr().cast(),
                (side * 4) as u32,
                0,
            );
            self.swap.Present(0, DXGI_PRESENT(0)).ok()
        }

        unsafe fn resize(&self, side: u32) -> windows::core::Result<()> {
            self.swap
                .ResizeBuffers(0, side, side, DXGI_FORMAT_UNKNOWN, DXGI_SWAP_CHAIN_FLAG(0))
        }
    }

    pub struct Overlay {
        hwnd: HWND,
        gpu: Option<Gpu>,
        /// Swap chain (and window) size: Bit's bitmap rounded up, so it
        /// isn't resized every frame as Bit grows and shrinks.
        cap: usize,
        clickable: bool,
        /// Clicks currently pass through the whole window.
        passing: bool,
        shown: bool,
    }

    impl Overlay {
        /// `clickable`: false lets every click pass straight through Bit.
        /// `hidden`: leave Bit out of screenshots, recordings and screen
        /// sharing.
        pub fn new(clickable: bool, hidden: bool) -> Option<Overlay> {
            let class = wide(CLASS);
            unsafe {
                let instance = GetModuleHandleW(std::ptr::null());
                static REGISTER: Once = Once::new();
                REGISTER.call_once(|| {
                    let wc = WNDCLASSW {
                        lpfnWndProc: Some(proc),
                        hInstance: instance,
                        hCursor: LoadCursorW(std::ptr::null_mut(), IDC_HAND),
                        lpszClassName: class.as_ptr(),
                        ..Default::default()
                    };
                    RegisterClassW(&wc);
                });
                // Clicks pass through until the pointer is over Bit itself.
                let hwnd = CreateWindowExW(
                    WS_EX_NOREDIRECTIONBITMAP
                        | WS_EX_TOOLWINDOW
                        | WS_EX_TOPMOST
                        | WS_EX_NOACTIVATE
                        | WS_EX_TRANSPARENT,
                    class.as_ptr(),
                    class.as_ptr(),
                    WS_POPUP,
                    0,
                    0,
                    1,
                    1,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    instance,
                    std::ptr::null(),
                );
                if hwnd.is_null() {
                    return None;
                }
                if hidden {
                    // Windows 10 2004 and later; ignored before that.
                    SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
                }
                DOWN.store(false, Ordering::Relaxed);
                RELEASED.store(false, Ordering::Relaxed);
                Some(Overlay {
                    hwnd,
                    gpu: None,
                    cap: 0,
                    clickable,
                    passing: true,
                    shown: false,
                })
            }
        }

        /// Show `pixels` (`side` x `side`, premultiplied BGRA) with its top
        /// left corner at desktop pixel (`x`, `y`).
        pub fn present(&mut self, x: i32, y: i32, side: usize, pixels: &[u32]) {
            unsafe {
                if side > self.cap || side + 256 < self.cap {
                    self.cap = side.next_multiple_of(64);
                    match &self.gpu {
                        Some(gpu) => {
                            if gpu.resize(self.cap as u32).is_err() {
                                self.gpu = None;
                            }
                        }
                        None => {}
                    }
                }
                if self.gpu.is_none() {
                    self.gpu = Gpu::new(self.hwnd, self.cap as u32).ok();
                }
                // Bit sits in the middle of its (rounded-up) window.
                let cap = self.cap as i32;
                let off = (cap - side as i32) / 2;
                let (wx, wy) = (x - off, y - off);
                SetWindowPos(
                    self.hwnd,
                    std::ptr::null_mut(),
                    wx,
                    wy,
                    cap,
                    cap,
                    SWP_NOACTIVATE | SWP_NOZORDER,
                );
                if let Some(gpu) = &self.gpu {
                    if gpu.show(side, off as u32, pixels).is_err() {
                        // Device lost (driver update, sleep...): start afresh next frame.
                        self.gpu = None;
                    }
                }
                if !self.shown {
                    ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
                    SetWindowPos(self.hwnd, HWND_TOPMOST, wx, wy, cap, cap, SWP_NOACTIVATE);
                    self.shown = true;
                }
                if self.clickable {
                    self.follow_pointer(x, y, side, pixels);
                }
            }
        }

        /// Catch clicks only while the pointer is over a visible part of Bit,
        /// so the see-through parts never block what's underneath.
        unsafe fn follow_pointer(&mut self, x: i32, y: i32, side: usize, pixels: &[u32]) {
            let mut p = POINT { x: 0, y: 0 };
            GetCursorPos(&mut p);
            let (lx, ly) = (p.x - x, p.y - y);
            // (Held down, it stays Bit's whatever is under the pointer.)
            let over = DOWN.load(Ordering::Relaxed)
                || (lx >= 0
                    && ly >= 0
                    && (lx as usize) < side
                    && (ly as usize) < side
                    && pixels[ly as usize * side + lx as usize] >> 24 > 16);
            if over == self.passing {
                self.passing = !over;
                let style = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE);
                let t = WS_EX_TRANSPARENT as isize;
                SetWindowLongPtrW(
                    self.hwnd,
                    GWL_EXSTYLE,
                    if over { style & !t } else { style | t },
                );
            }
        }

        /// Handle the window's messages; whether the button is down on Bit
        /// now, and whether it has been let go since last time.
        pub fn press(&mut self) -> (bool, bool) {
            unsafe {
                let mut msg: MSG = std::mem::zeroed();
                while PeekMessageW(&mut msg, self.hwnd, 0, 0, PM_REMOVE) != 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            (
                DOWN.load(Ordering::Relaxed),
                RELEASED.swap(false, Ordering::Relaxed),
            )
        }

        /// Wait for the display's next refresh.
        pub fn wait_frame(&self) {
            if unsafe { DwmFlush() } != 0 {
                std::thread::sleep(std::time::Duration::from_millis(16));
            }
        }
    }

    impl Drop for Overlay {
        fn drop(&mut self) {
            unsafe {
                if self.shown {
                    ShowWindow(self.hwnd, SW_HIDE);
                }
                self.gpu = None;
                DestroyWindow(self.hwnd);
            }
        }
    }
}

#[cfg(windows)]
mod layered {
    use crate::platform::wide;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Once;
    use windows_sys::Win32::{
        Foundation::{HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM},
        Graphics::Dwm::DwmFlush,
        Graphics::Gdi::{
            CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC,
            SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS,
            HBITMAP, HDC, HGDIOBJ,
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture},
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, LoadCursorW,
            PeekMessageW, RegisterClassW, SetWindowPos, ShowWindow, TranslateMessage,
            UpdateLayeredWindow, HWND_TOPMOST, IDC_HAND, MA_NOACTIVATE, MSG, PM_REMOVE,
            SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SW_HIDE, SW_SHOWNOACTIVATE, ULW_ALPHA,
            WM_CAPTURECHANGED, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEACTIVATE, WNDCLASSW,
            WS_EX_LAYERED, WS_EX_NOACTIVATE,
            WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
        },
    };

    const CLASS: &str = "TronBitLayeredOverlay";
    static DOWN: AtomicBool = AtomicBool::new(false);
    static RELEASED: AtomicBool = AtomicBool::new(false);

    unsafe extern "system" fn proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        match msg {
            WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
            WM_LBUTTONDOWN => {
                DOWN.store(true, Ordering::Relaxed);
                SetCapture(hwnd);
                0
            }
            WM_LBUTTONUP => {
                let held = DOWN.swap(false, Ordering::Relaxed);
                RELEASED.fetch_or(held, Ordering::Relaxed);
                ReleaseCapture();
                0
            }
            WM_CAPTURECHANGED => {
                let held = DOWN.swap(false, Ordering::Relaxed);
                RELEASED.fetch_or(held, Ordering::Relaxed);
                0
            }
            _ => DefWindowProcW(hwnd, msg, w, l),
        }
    }

    pub struct Overlay {
        hwnd: HWND,
        screen: HDC,
        mem: HDC,
        bitmap: HBITMAP,
        old: HGDIOBJ,
        bits: *mut u32,
        cap: usize,
        shown: bool,
    }

    impl Overlay {
        /// `clickable`: false lets every click pass straight through Bit.
        pub fn new(clickable: bool) -> Option<Overlay> {
            let class = wide(CLASS);
            unsafe {
                let instance = GetModuleHandleW(std::ptr::null());
                static REGISTER: Once = Once::new();
                REGISTER.call_once(|| {
                    let wc = WNDCLASSW {
                        lpfnWndProc: Some(proc),
                        hInstance: instance,
                        hCursor: LoadCursorW(std::ptr::null_mut(), IDC_HAND),
                        lpszClassName: class.as_ptr(),
                        ..Default::default()
                    };
                    RegisterClassW(&wc);
                });
                let hwnd = CreateWindowExW(
                    WS_EX_LAYERED
                        | WS_EX_TOOLWINDOW
                        | WS_EX_TOPMOST
                        | WS_EX_NOACTIVATE
                        | if clickable { 0 } else { WS_EX_TRANSPARENT },
                    class.as_ptr(),
                    class.as_ptr(),
                    WS_POPUP,
                    0,
                    0,
                    1,
                    1,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    instance,
                    std::ptr::null(),
                );
                if hwnd.is_null() {
                    return None;
                }
                let screen = GetDC(std::ptr::null_mut());
                let mem = CreateCompatibleDC(screen);
                DOWN.store(false, Ordering::Relaxed);
                RELEASED.store(false, Ordering::Relaxed);
                Some(Overlay {
                    hwnd,
                    screen,
                    mem,
                    bitmap: std::ptr::null_mut(),
                    old: std::ptr::null_mut(),
                    bits: std::ptr::null_mut(),
                    cap: 0,
                    shown: false,
                })
            }
        }

        /// Make sure the bitmap holds `side` x `side` pixels.
        unsafe fn reserve(&mut self, side: usize) -> bool {
            if side <= self.cap && !self.bits.is_null() {
                return true;
            }
            let cap = side.next_multiple_of(64);
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: cap as i32,
                    biHeight: -(cap as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let bitmap = CreateDIBSection(
                self.screen,
                &info,
                DIB_RGB_COLORS,
                &mut bits,
                std::ptr::null_mut(),
                0,
            );
            if bitmap.is_null() || bits.is_null() {
                return false;
            }
            let old = SelectObject(self.mem, bitmap);
            if self.bitmap.is_null() {
                self.old = old;
            } else {
                DeleteObject(self.bitmap);
            }
            self.bitmap = bitmap;
            self.bits = bits.cast();
            self.cap = cap;
            true
        }

        /// Show `pixels` (`side` x `side`, premultiplied BGRA) with its top
        /// left corner at desktop pixel (`x`, `y`).
        pub fn present(&mut self, x: i32, y: i32, side: usize, pixels: &[u32]) {
            unsafe {
                if !self.reserve(side) {
                    return;
                }
                for row in 0..side {
                    std::ptr::copy_nonoverlapping(
                        pixels.as_ptr().add(row * side),
                        self.bits.add(row * self.cap),
                        side,
                    );
                }
                let dst = POINT { x, y };
                let src = POINT { x: 0, y: 0 };
                let size = SIZE {
                    cx: side as i32,
                    cy: side as i32,
                };
                let blend = BLENDFUNCTION {
                    BlendOp: 0,
                    BlendFlags: 0,
                    SourceConstantAlpha: 255,
                    AlphaFormat: 1,
                };
                UpdateLayeredWindow(
                    self.hwnd,
                    self.screen,
                    &dst,
                    &size,
                    self.mem,
                    &src,
                    0,
                    &blend,
                    ULW_ALPHA,
                );
                if !self.shown {
                    ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
                    SetWindowPos(
                        self.hwnd,
                        HWND_TOPMOST,
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                    );
                    self.shown = true;
                }
            }
        }

        /// Handle the window's messages; whether the button is down on Bit
        /// now, and whether it has been let go since last time.
        pub fn press(&mut self) -> (bool, bool) {
            unsafe {
                let mut msg: MSG = std::mem::zeroed();
                while PeekMessageW(&mut msg, self.hwnd, 0, 0, PM_REMOVE) != 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            (
                DOWN.load(Ordering::Relaxed),
                RELEASED.swap(false, Ordering::Relaxed),
            )
        }

        /// Wait for the display's next refresh.
        pub fn wait_frame(&self) {
            if unsafe { DwmFlush() } != 0 {
                std::thread::sleep(std::time::Duration::from_millis(16));
            }
        }
    }

    impl Drop for Overlay {
        fn drop(&mut self) {
            unsafe {
                if self.shown {
                    ShowWindow(self.hwnd, SW_HIDE);
                }
                DestroyWindow(self.hwnd);
                if !self.bitmap.is_null() {
                    SelectObject(self.mem, self.old);
                    DeleteObject(self.bitmap);
                }
                DeleteDC(self.mem);
                ReleaseDC(std::ptr::null_mut(), self.screen);
            }
        }
    }
}

/// Windows: the lean layered window normally, or the DirectComposition one
/// when Bit is to be left out of screenshots (Windows won't hide a layered
/// window from capture).
#[cfg(windows)]
mod win {
    pub enum Overlay {
        Layered(super::layered::Overlay),
        Composed(super::composed::Overlay),
    }

    impl Overlay {
        pub fn new(
            _shared: &std::sync::Arc<crate::Shared>,
            clickable: bool,
            hidden: bool,
        ) -> Option<Overlay> {
            if hidden {
                if let Some(o) = super::composed::Overlay::new(clickable, true) {
                    return Some(Overlay::Composed(o));
                }
            }
            super::layered::Overlay::new(clickable).map(Overlay::Layered)
        }

        pub fn present(&mut self, x: i32, y: i32, side: usize, pixels: &[u32]) {
            match self {
                Overlay::Layered(o) => o.present(x, y, side, pixels),
                Overlay::Composed(o) => o.present(x, y, side, pixels),
            }
        }

        pub fn press(&mut self) -> (bool, bool) {
            match self {
                Overlay::Layered(o) => o.press(),
                Overlay::Composed(o) => o.press(),
            }
        }

        pub fn wait_frame(&self) {
            match self {
                Overlay::Layered(o) => o.wait_frame(),
                Overlay::Composed(o) => o.wait_frame(),
            }
        }
    }
}

#[cfg(not(windows))]
mod other {
    use std::sync::Arc;

    /// One frame of Bit for the settings app to put on screen.
    pub struct OverlayFrame {
        pub x: f32,
        pub y: f32,
        pub side: usize,
        pub pixels: Vec<u32>,
    }

    pub struct Overlay {
        shared: Arc<crate::Shared>,
    }

    impl Overlay {
        pub fn new(
            shared: &Arc<crate::Shared>,
            _clickable: bool,
            _hidden: bool,
        ) -> Option<Overlay> {
            Some(Overlay {
                shared: shared.clone(),
            })
        }

        pub fn present(&mut self, x: i32, y: i32, side: usize, pixels: &[u32]) {
            if let Ok(mut f) = self.shared.overlay.lock() {
                *f = Some(OverlayFrame {
                    x: x as f32,
                    y: y as f32,
                    side,
                    pixels: pixels.to_vec(),
                });
            }
            if let Some(ctx) = self.shared.egui.lock().unwrap().as_ref() {
                ctx.request_repaint();
            }
        }

        pub fn press(&mut self) -> (bool, bool) {
            (false, false)
        }

        pub fn wait_frame(&self) {
            std::thread::sleep(std::time::Duration::from_millis(16));
        }
    }

    impl Drop for Overlay {
        fn drop(&mut self) {
            if let Ok(mut f) = self.shared.overlay.lock() {
                *f = None;
            }
            if let Some(ctx) = self.shared.egui.lock().unwrap().as_ref() {
                ctx.request_repaint();
            }
        }
    }
}
