use std::path::PathBuf;

#[derive(Clone)]
pub struct LauncherConfig {
    pub url: String,
    pub log_path: PathBuf,
}

#[cfg(windows)]
mod logo;
#[cfg(windows)]
mod paint;

#[cfg(windows)]
mod imp {
    use super::logo::{create_logo_hicon, render_logo_bgra};
    use super::paint::{make_font, paint_window};
    use super::LauncherConfig;
    use crate::open_browser;
    use crate::supervisor::SupervisorState;
    use std::cell::Cell;
    use std::ffi::OsStr;
    use std::mem::size_of;
    use std::os::windows::ffi::OsStrExt;
    use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
    use std::sync::{Arc, Mutex};
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{BOOL, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
    use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWINDOWATTRIBUTE};
    use windows::Win32::Graphics::Gdi::{
        DeleteObject, GetDC, GetDeviceCaps, InvalidateRect, ReleaseDC, FW_MEDIUM, FW_NORMAL,
        FW_SEMIBOLD, HFONT, LOGPIXELSX,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::HiDpi::{
        SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_ESCAPE, VK_RETURN, VK_SPACE, VK_TAB,
    };
    use windows::Win32::UI::Shell::{
        Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        AdjustWindowRectEx, AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW,
        DestroyIcon, DestroyMenu, DestroyWindow, DispatchMessageW, GetCursorPos, GetMessageW,
        GetSystemMetrics, GetWindowLongPtrW, IsWindowVisible, LoadCursorW, PostMessageW,
        PostQuitMessage, RegisterClassExW, SetCursor, SetForegroundWindow, SetTimer,
        SetWindowLongPtrW, ShowWindow, TrackPopupMenu, TranslateMessage, CREATESTRUCTW,
        GWLP_USERDATA, HICON, IDC_ARROW, IDC_HAND, MF_SEPARATOR, MF_STRING, MSG, SM_CXSCREEN,
        SM_CYSCREEN, SW_HIDE, SW_RESTORE, SW_SHOW, TPM_BOTTOMALIGN, TPM_LEFTALIGN, WM_APP,
        WM_CLOSE, WM_COMMAND, WM_CREATE, WM_DESTROY, WM_KEYDOWN, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN,
        WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCCREATE, WM_NCDESTROY, WM_PAINT, WM_RBUTTONUP,
        WM_SETCURSOR, WM_TIMER, WNDCLASSEXW, WS_CAPTION, WS_MINIMIZEBOX, WS_OVERLAPPED,
        WS_SYSMENU,
    };

    const LOGICAL_W: i32 = 440;
    const LOGICAL_H: i32 = 286;
    const TIMER_REFRESH_ID: usize = 1;
    const WM_MOUSELEAVE_MSG: u32 = 0x02A3;
    const WM_TRAY_CALLBACK: u32 = WM_APP + 1;
    const WM_APP_SHUTDOWN: u32 = WM_APP + 2;

    const CMD_OPEN_WEB: usize = 1001;
    const CMD_SHOW_WINDOW: usize = 1002;
    const CMD_OPEN_LOG: usize = 1003;
    const CMD_STOP_SERVER: usize = 1004;

    pub(super) const BTN_OPEN_WEB: usize = 0;
    pub(super) const BTN_HIDE_TRAY: usize = 1;
    pub(super) const BTN_STOP_SERVER: usize = 2;

    fn wide(s: &str) -> Vec<u16> {
        OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
    }

    #[derive(Clone)]
    pub(super) struct LiveSummary {
        pub(super) show_title: String,
        pub(super) active_channels: usize,
        pub(super) total_channels: usize,
        pub(super) stopping: bool,
    }

    pub(super) struct LauncherState {
        pub(super) sup: Arc<SupervisorState>,
        pub(super) cfg: LauncherConfig,
        pub(super) dpi: i32,
        pub(super) summary: Mutex<LiveSummary>,
        pub(super) hover_btn: Cell<Option<usize>>,
        pub(super) pressed_btn: Cell<Option<usize>>,
        pub(super) focused_btn: Cell<Option<usize>>,
        pub(super) tracking_mouse: Cell<bool>,
        pub(super) tray_added: Cell<bool>,
        pub(super) icon_large: HICON,
        pub(super) icon_small: HICON,
        pub(super) font_title: HFONT,
        pub(super) font_sub: HFONT,
        pub(super) font_label: HFONT,
        pub(super) font_mono: HFONT,
        pub(super) font_btn: HFONT,
        pub(super) font_caption: HFONT,
        pub(super) logo_pixels: Vec<u8>,
        pub(super) logo_size: i32,
        pub(super) stopping: AtomicBool,
    }

    impl Drop for LauncherState {
        fn drop(&mut self) {
            unsafe {
                let _ = DeleteObject(self.font_title);
                let _ = DeleteObject(self.font_sub);
                let _ = DeleteObject(self.font_label);
                let _ = DeleteObject(self.font_mono);
                let _ = DeleteObject(self.font_btn);
                let _ = DeleteObject(self.font_caption);
                if !self.icon_large.is_invalid() {
                    let _ = DestroyIcon(self.icon_large);
                }
                if !self.icon_small.is_invalid() {
                    let _ = DestroyIcon(self.icon_small);
                }
            }
        }
    }

    impl LauncherState {
        pub(super) fn s(&self, logical_px: i32) -> i32 {
            (logical_px * self.dpi + 48) / 96
        }

        pub(super) fn button_rects(&self) -> [RECT; 3] {
            let pad = self.s(20);
            let gap = self.s(10);
            let w = self.s(LOGICAL_W);
            let y1 = self.s(206);
            let y2 = y1 + self.s(36);
            let open_w = self.s(146);
            let hide_w = self.s(128);
            let stop_left = pad + open_w + gap + hide_w + gap;
            [
                RECT { left: pad, top: y1, right: pad + open_w, bottom: y2 },
                RECT { left: pad + open_w + gap, top: y1, right: pad + open_w + gap + hide_w, bottom: y2 },
                RECT { left: stop_left, top: y1, right: w - pad, bottom: y2 },
            ]
        }

        pub(super) fn hit_test(&self, x: i32, y: i32) -> Option<usize> {
            self.button_rects()
                .iter()
                .position(|r| x >= r.left && x <= r.right && y >= r.top && y <= r.bottom)
        }
    }

    fn refresh_summary(st: &LauncherState) {
        let Ok(show) = st.sup.show.try_read() else { return };
        let Ok(channels) = st.sup.channels.try_read() else { return };
        let active_channels = channels.values().filter(|c| c.config.enabled).count();
        let next = LiveSummary {
            show_title: show.title.clone(),
            active_channels,
            total_channels: channels.len(),
            stopping: st.stopping.load(Ordering::Acquire),
        };
        if let Ok(mut guard) = st.summary.lock() {
            *guard = next;
        }
    }

    unsafe fn trigger_stop(hwnd: HWND, st: &LauncherState) {
        if !st.stopping.swap(true, Ordering::AcqRel) {
            if let Ok(mut g) = st.summary.lock() {
                g.stopping = true;
            }
            unsafe {
                let _ = InvalidateRect(hwnd, None, BOOL(0));
            }
            st.sup.request_shutdown();
        }
    }

    unsafe fn add_tray_icon(hwnd: HWND, st: &LauncherState) {
        let mut nid = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: 1,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
            uCallbackMessage: WM_TRAY_CALLBACK,
            hIcon: st.icon_small,
            ..Default::default()
        };
        let tip = format!("PGMWatch ({})", st.cfg.url);
        for (i, c) in tip.encode_utf16().take(127).enumerate() {
            nid.szTip[i] = c;
        }
        unsafe {
            st.tray_added.set(Shell_NotifyIconW(NIM_ADD, &nid).as_bool());
        }
    }

    unsafe fn remove_tray_icon(hwnd: HWND, st: &LauncherState) {
        if st.tray_added.get() {
            let nid = NOTIFYICONDATAW {
                cbSize: size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: hwnd,
                uID: 1,
                ..Default::default()
            };
            unsafe {
                let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
            }
            st.tray_added.set(false);
        }
    }

    unsafe fn show_tray_menu(hwnd: HWND) {
        unsafe {
            let Ok(menu) = CreatePopupMenu() else { return };
            let l1 = wide("Open Web Console");
            let l2 = wide("Show Control Window");
            let l3 = wide("Open Log File");
            let l4 = wide("Stop Server & Exit");
            let _ = AppendMenuW(menu, MF_STRING, CMD_OPEN_WEB, PCWSTR(l1.as_ptr()));
            let _ = AppendMenuW(menu, MF_STRING, CMD_SHOW_WINDOW, PCWSTR(l2.as_ptr()));
            let _ = AppendMenuW(menu, MF_STRING, CMD_OPEN_LOG, PCWSTR(l3.as_ptr()));
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            let _ = AppendMenuW(menu, MF_STRING, CMD_STOP_SERVER, PCWSTR(l4.as_ptr()));
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            let _ = SetForegroundWindow(hwnd);
            let _ = TrackPopupMenu(menu, TPM_LEFTALIGN | TPM_BOTTOMALIGN, pt.x, pt.y, 0, hwnd, None);
            let _ = DestroyMenu(menu);
        }
    }

    unsafe fn invoke_button(hwnd: HWND, st: &LauncherState, idx: usize) {
        match idx {
            BTN_OPEN_WEB => open_browser(&st.cfg.url),
            BTN_HIDE_TRAY => unsafe {
                if !st.tray_added.get() {
                    add_tray_icon(hwnd, st);
                }
                if st.tray_added.get() {
                    let _ = ShowWindow(hwnd, SW_HIDE);
                }
            },
            BTN_STOP_SERVER => unsafe { trigger_stop(hwnd, st) },
            _ => {}
        }
    }

    unsafe extern "system" fn wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        unsafe {
            if msg == WM_NCCREATE {
                let cs = &*(lparam.0 as *const CREATESTRUCTW);
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            }

            if msg == WM_TRAY_CALLBACK {
                let ev = (lparam.0 & 0xffff) as u32;
                if ev == WM_LBUTTONUP || ev == WM_LBUTTONDBLCLK {
                    let _ = ShowWindow(hwnd, SW_SHOW);
                    let _ = ShowWindow(hwnd, SW_RESTORE);
                    let _ = SetForegroundWindow(hwnd);
                } else if ev == WM_RBUTTONUP {
                    show_tray_menu(hwnd);
                }
                return LRESULT(0);
            }

            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut LauncherState;
            if ptr.is_null() {
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            }
            let st = &*ptr;

            match msg {
                WM_CREATE => {
                    let dark: i32 = 1;
                    let _ = DwmSetWindowAttribute(
                        hwnd,
                        DWMWINDOWATTRIBUTE(20),
                        &dark as *const i32 as *const _,
                        4,
                    );
                    let caption_bgr: u32 = 0x0013_100E;
                    let _ = DwmSetWindowAttribute(
                        hwnd,
                        DWMWINDOWATTRIBUTE(35),
                        &caption_bgr as *const u32 as *const _,
                        4,
                    );
                    add_tray_icon(hwnd, st);
                    refresh_summary(st);
                    let _ = SetTimer(hwnd, TIMER_REFRESH_ID, 1000, None);
                    LRESULT(0)
                }
                WM_PAINT => {
                    paint_window(hwnd, st);
                    LRESULT(0)
                }
                WM_TIMER => {
                    refresh_summary(st);
                    if IsWindowVisible(hwnd).as_bool() {
                        let _ = InvalidateRect(hwnd, None, BOOL(0));
                    }
                    LRESULT(0)
                }
                WM_MOUSEMOVE => {
                    if !st.tracking_mouse.get() {
                        let mut tme = TRACKMOUSEEVENT {
                            cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                            dwFlags: TME_LEAVE,
                            hwndTrack: hwnd,
                            dwHoverTime: 0,
                        };
                        st.tracking_mouse.set(TrackMouseEvent(&mut tme).is_ok());
                    }
                    let x = (lparam.0 & 0xffff) as i16 as i32;
                    let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
                    let hit = st.hit_test(x, y);
                    if hit != st.hover_btn.get() {
                        st.hover_btn.set(hit);
                        let _ = InvalidateRect(hwnd, None, BOOL(0));
                    }
                    LRESULT(0)
                }
                WM_MOUSELEAVE_MSG => {
                    st.tracking_mouse.set(false);
                    if st.hover_btn.get().is_some() || st.pressed_btn.get().is_some() {
                        st.hover_btn.set(None);
                        st.pressed_btn.set(None);
                        let _ = InvalidateRect(hwnd, None, BOOL(0));
                    }
                    LRESULT(0)
                }
                WM_SETCURSOR => {
                    let hit_test = (lparam.0 & 0xffff) as u32;
                    if hit_test == 1 && st.hover_btn.get().is_some() {
                        let _ = SetCursor(LoadCursorW(None, IDC_HAND).unwrap_or_default());
                        return LRESULT(1);
                    }
                    DefWindowProcW(hwnd, msg, wparam, lparam)
                }
                WM_LBUTTONDOWN => {
                    let x = (lparam.0 & 0xffff) as i16 as i32;
                    let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
                    if let Some(hit) = st.hit_test(x, y) {
                        st.pressed_btn.set(Some(hit));
                        st.focused_btn.set(Some(hit));
                        let _ = InvalidateRect(hwnd, None, BOOL(0));
                    }
                    LRESULT(0)
                }
                WM_LBUTTONUP => {
                    let x = (lparam.0 & 0xffff) as i16 as i32;
                    let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
                    let hit = st.hit_test(x, y);
                    let prev = st.pressed_btn.take();
                    let _ = InvalidateRect(hwnd, None, BOOL(0));
                    if let (Some(a), Some(b)) = (prev, hit)
                        && a == b
                    {
                        invoke_button(hwnd, st, a);
                    }
                    LRESULT(0)
                }
                WM_KEYDOWN => {
                    let vk = wparam.0 as u16;
                    if vk == VK_TAB.0 {
                        st.focused_btn.set(Some(match st.focused_btn.get() {
                            None => 0,
                            Some(i) => (i + 1) % 3,
                        }));
                        let _ = InvalidateRect(hwnd, None, BOOL(0));
                    } else if vk == VK_RETURN.0 || vk == VK_SPACE.0 {
                        let idx = st.focused_btn.get().unwrap_or(BTN_OPEN_WEB);
                        invoke_button(hwnd, st, idx);
                    } else if vk == VK_ESCAPE.0 && st.focused_btn.get().is_some() {
                        st.focused_btn.set(None);
                        let _ = InvalidateRect(hwnd, None, BOOL(0));
                    }
                    LRESULT(0)
                }
                WM_COMMAND => {
                    let cmd = wparam.0 & 0xffff;
                    match cmd {
                        CMD_OPEN_WEB => open_browser(&st.cfg.url),
                        CMD_SHOW_WINDOW => {
                            let _ = ShowWindow(hwnd, SW_SHOW);
                            let _ = ShowWindow(hwnd, SW_RESTORE);
                            let _ = SetForegroundWindow(hwnd);
                        }
                        CMD_OPEN_LOG => {
                            if !st.cfg.log_path.exists() {
                                if let Some(parent) = st.cfg.log_path.parent() {
                                    let _ = std::fs::create_dir_all(parent);
                                }
                                let _ = std::fs::OpenOptions::new()
                                    .create(true)
                                    .append(true)
                                    .open(&st.cfg.log_path);
                            }
                            let _ = std::process::Command::new("notepad.exe")
                                .arg(&st.cfg.log_path)
                                .spawn();
                        }
                        CMD_STOP_SERVER => trigger_stop(hwnd, st),
                        _ => {}
                    }
                    LRESULT(0)
                }
                WM_CLOSE => {
                    trigger_stop(hwnd, st);
                    LRESULT(0)
                }
                WM_APP_SHUTDOWN => {
                    remove_tray_icon(hwnd, st);
                    let _ = DestroyWindow(hwnd);
                    LRESULT(0)
                }
                WM_DESTROY => {
                    remove_tray_icon(hwnd, st);
                    PostQuitMessage(0);
                    LRESULT(0)
                }
                WM_NCDESTROY => {
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                    drop(Box::from_raw(ptr));
                    DefWindowProcW(hwnd, msg, wparam, lparam)
                }
                _ => DefWindowProcW(hwnd, msg, wparam, lparam),
            }
        }
    }

    pub fn run_launcher_window(sup: Arc<SupervisorState>, cfg: LauncherConfig) {
        let rt = tokio::runtime::Handle::current();
        let handle = std::thread::Builder::new()
            .name("pgmwatch-tray-ui".to_string())
            .spawn(move || unsafe {
                let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
                let h_inst: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
                let hdc_screen = GetDC(None);
                let dpi = GetDeviceCaps(hdc_screen, LOGPIXELSX).max(96);
                ReleaseDC(None, hdc_screen);
                let scale = |v: i32| (v * dpi + 48) / 96;

                let icon_large = create_logo_hicon(scale(48));
                let icon_small = create_logo_hicon(scale(20));
                let logo_size = scale(40);
                let logo_pixels = render_logo_bgra(logo_size, Some([19, 21, 25]));

                let state = Box::new(LauncherState {
                    sup: Arc::clone(&sup),
                    cfg,
                    dpi,
                    summary: Mutex::new(LiveSummary {
                        show_title: "PGMWatch".to_string(),
                        active_channels: 0,
                        total_channels: 0,
                        stopping: false,
                    }),
                    hover_btn: Cell::new(None),
                    pressed_btn: Cell::new(None),
                    focused_btn: Cell::new(None),
                    tracking_mouse: Cell::new(false),
                    tray_added: Cell::new(false),
                    icon_large,
                    icon_small,
                    font_title: make_font(scale(19), FW_SEMIBOLD.0, "Segoe UI"),
                    font_sub: make_font(scale(12), FW_NORMAL.0, "Segoe UI"),
                    font_label: make_font(scale(13), FW_MEDIUM.0, "Segoe UI"),
                    font_mono: make_font(scale(13), FW_NORMAL.0, "Consolas"),
                    font_btn: make_font(scale(13), FW_SEMIBOLD.0, "Segoe UI"),
                    font_caption: make_font(scale(11), FW_MEDIUM.0, "Segoe UI"),
                    logo_pixels,
                    logo_size,
                    stopping: AtomicBool::new(false),
                });

                let class_name = wide("PGMWatchLauncherClass");
                let wc = WNDCLASSEXW {
                    cbSize: size_of::<WNDCLASSEXW>() as u32,
                    hInstance: h_inst,
                    hIcon: icon_large,
                    hIconSm: icon_small,
                    hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                    lpszClassName: PCWSTR(class_name.as_ptr()),
                    lpfnWndProc: Some(wnd_proc),
                    ..Default::default()
                };
                RegisterClassExW(&wc);

                let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
                let mut adj = RECT {
                    left: 0,
                    top: 0,
                    right: scale(LOGICAL_W),
                    bottom: scale(LOGICAL_H),
                };
                let _ = AdjustWindowRectEx(&mut adj, style, BOOL(0), Default::default());
                let win_w = adj.right - adj.left;
                let win_h = adj.bottom - adj.top;
                let x = ((GetSystemMetrics(SM_CXSCREEN) - win_w) / 2).max(0);
                let y = ((GetSystemMetrics(SM_CYSCREEN) - win_h) / 2).max(0);

                let win_title = wide("PGMWatch — Server Control");
                let state_ptr = Box::into_raw(state);
                let Ok(hwnd) = CreateWindowExW(
                    Default::default(),
                    PCWSTR(class_name.as_ptr()),
                    PCWSTR(win_title.as_ptr()),
                    style,
                    x,
                    y,
                    win_w,
                    win_h,
                    None,
                    None,
                    h_inst,
                    Some(state_ptr as *const _),
                ) else {
                    drop(Box::from_raw(state_ptr));
                    sup.request_shutdown();
                    return;
                };

                let hwnd_raw = Arc::new(AtomicIsize::new(hwnd.0 as isize));
                let hwnd_watch = Arc::clone(&hwnd_raw);
                let mut shutdown_rx = sup.subscribe_shutdown();
                rt.spawn(async move {
                    while !*shutdown_rx.borrow() {
                        if shutdown_rx.changed().await.is_err() {
                            break;
                        }
                    }
                    let raw = hwnd_watch.load(Ordering::Acquire);
                    if raw != 0 {
                        let _ = PostMessageW(
                            HWND(raw as *mut _),
                            WM_APP_SHUTDOWN,
                            WPARAM(0),
                            LPARAM(0),
                        );
                    }
                });

                let _ = ShowWindow(hwnd, SW_SHOW);

                let mut msg = MSG::default();
                while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                hwnd_raw.store(0, Ordering::Release);
            });

        if let Err(err) = handle {
            tracing::warn!("Failed to spawn PGMWatch Windows launcher UI: {err}");
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::LauncherConfig;
    use crate::supervisor::SupervisorState;
    use std::sync::Arc;

    pub fn run_launcher_window(_sup: Arc<SupervisorState>, _cfg: LauncherConfig) {}
}

pub use imp::run_launcher_window;
