use super::imp::{LauncherState, LiveSummary, BTN_HIDE_TRAY, BTN_OPEN_WEB};
use std::ffi::OsStr;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use windows::Win32::Foundation::{COLORREF, HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontIndirectW,
    CreatePen, CreateSolidBrush, DeleteDC, DeleteObject, DrawTextW, EndPaint, FillRect, RoundRect,
    SelectObject, SetBkMode, SetTextColor, StretchDIBits, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH, DIB_RGB_COLORS,
    DRAW_TEXT_FORMAT, DT_CENTER, DT_END_ELLIPSIS, DT_LEFT, DT_RIGHT, DT_SINGLELINE, DT_VCENTER,
    FF_DONTCARE, HFONT, LOGFONTW, OUT_DEFAULT_PRECIS, PAINTSTRUCT, PS_SOLID, SRCCOPY, TRANSPARENT,
};
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;

pub(super) const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF(((b as u32) << 16) | ((g as u32) << 8) | (r as u32))
}

pub(super) fn make_font(height: i32, weight: u32, family: &str) -> HFONT {
    let mut name_buf = [0u16; 32];
    for (i, ch) in family.encode_utf16().take(31).enumerate() {
        name_buf[i] = ch;
    }
    let lf = LOGFONTW {
        lfHeight: -height,
        lfWeight: weight as i32,
        lfCharSet: DEFAULT_CHARSET,
        lfOutPrecision: OUT_DEFAULT_PRECIS,
        lfClipPrecision: CLIP_DEFAULT_PRECIS,
        lfQuality: CLEARTYPE_QUALITY,
        lfPitchAndFamily: DEFAULT_PITCH.0 | FF_DONTCARE.0,
        lfFaceName: name_buf,
        ..Default::default()
    };
    unsafe { CreateFontIndirectW(&lf) }
}

unsafe fn draw_rounded_box(
    hdc: windows::Win32::Graphics::Gdi::HDC,
    r: RECT,
    radius: i32,
    fill: COLORREF,
    stroke: COLORREF,
) {
    unsafe {
        let brush = CreateSolidBrush(fill);
        let pen = CreatePen(PS_SOLID, 1, stroke);
        let old_b = SelectObject(hdc, brush);
        let old_p = SelectObject(hdc, pen);
        let _ = RoundRect(hdc, r.left, r.top, r.right, r.bottom, radius, radius);
        SelectObject(hdc, old_b);
        SelectObject(hdc, old_p);
        let _ = DeleteObject(brush);
        let _ = DeleteObject(pen);
    }
}

unsafe fn draw_text(
    hdc: windows::Win32::Graphics::Gdi::HDC,
    text: &str,
    mut rect: RECT,
    color: COLORREF,
    font: HFONT,
    flags: u32,
) {
    unsafe {
        let old_f = SelectObject(hdc, font);
        SetTextColor(hdc, color);
        let mut w: Vec<u16> = OsStr::new(text).encode_wide().collect();
        if !w.is_empty() {
            let _ = DrawTextW(hdc, &mut w, &mut rect, DRAW_TEXT_FORMAT(flags));
        }
        SelectObject(hdc, old_f);
    }
}

pub(super) unsafe fn paint_window(hwnd: HWND, st: &LauncherState) {
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let hdc_win = BeginPaint(hwnd, &mut ps);
        let mut client = RECT::default();
        let _ = GetClientRect(hwnd, &mut client);
        let w = (client.right - client.left).max(1);
        let h = (client.bottom - client.top).max(1);

        let hdc = CreateCompatibleDC(hdc_win);
        let bmp = CreateCompatibleBitmap(hdc_win, w, h);
        let old_bmp = SelectObject(hdc, bmp);
        SetBkMode(hdc, TRANSPARENT);

        let bg_brush = CreateSolidBrush(rgb(19, 21, 25));
        let _ = FillRect(hdc, &client, bg_brush);
        let _ = DeleteObject(bg_brush);

        let pad = st.s(20);
        let summary = st.summary.lock().map(|g| g.clone()).unwrap_or(LiveSummary {
            show_title: "PGMWatch".to_string(),
            active_channels: 0,
            total_channels: 0,
            stopping: false,
        });

        let logo_x = pad;
        let logo_y = st.s(22);
        let bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: st.logo_size,
                biHeight: -st.logo_size,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let _ = StretchDIBits(
            hdc,
            logo_x,
            logo_y,
            st.logo_size,
            st.logo_size,
            0,
            0,
            st.logo_size,
            st.logo_size,
            Some(st.logo_pixels.as_ptr() as *const _),
            &bi,
            DIB_RGB_COLORS,
            SRCCOPY,
        );

        let text_left = logo_x + st.logo_size + st.s(12);
        draw_text(
            hdc,
            "PGMWatch",
            RECT { left: text_left, top: st.s(20), right: w - st.s(120), bottom: st.s(44) },
            rgb(238, 240, 244),
            st.font_title,
            DT_LEFT.0 | DT_VCENTER.0 | DT_SINGLELINE.0,
        );
        draw_text(
            hdc,
            "Broadcast Confidence Monitor",
            RECT { left: text_left, top: st.s(44), right: w - st.s(120), bottom: st.s(62) },
            rgb(132, 139, 152),
            st.font_sub,
            DT_LEFT.0 | DT_VCENTER.0 | DT_SINGLELINE.0,
        );

        let pill = RECT { left: w - pad - st.s(88), top: st.s(28), right: w - pad, bottom: st.s(52) };
        if summary.stopping {
            draw_rounded_box(hdc, pill, st.s(12), rgb(52, 24, 27), rgb(127, 41, 46));
            draw_text(
                hdc,
                "STOPPING",
                pill,
                rgb(252, 165, 165),
                st.font_caption,
                DT_CENTER.0 | DT_VCENTER.0 | DT_SINGLELINE.0,
            );
        } else {
            draw_rounded_box(hdc, pill, st.s(12), rgb(18, 49, 33), rgb(33, 92, 61));
            let dot = RECT {
                left: pill.left + st.s(10),
                top: pill.top + st.s(8),
                right: pill.left + st.s(18),
                bottom: pill.top + st.s(16),
            };
            draw_rounded_box(hdc, dot, st.s(8), rgb(21, 201, 101), rgb(21, 201, 101));
            draw_text(
                hdc,
                "RUNNING",
                RECT {
                    left: pill.left + st.s(16),
                    top: pill.top,
                    right: pill.right - st.s(4),
                    bottom: pill.bottom,
                },
                rgb(110, 231, 183),
                st.font_caption,
                DT_CENTER.0 | DT_VCENTER.0 | DT_SINGLELINE.0,
            );
        }

        let card = RECT { left: pad, top: st.s(80), right: w - pad, bottom: st.s(188) };
        draw_rounded_box(hdc, card, st.s(12), rgb(24, 27, 32), rgb(42, 47, 58));

        let c_pad = st.s(14);
        draw_text(
            hdc,
            "Web Console",
            RECT { left: card.left + c_pad, top: card.top + st.s(12), right: card.left + st.s(120), bottom: card.top + st.s(32) },
            rgb(132, 139, 152),
            st.font_label,
            DT_LEFT.0 | DT_VCENTER.0 | DT_SINGLELINE.0,
        );
        draw_text(
            hdc,
            &st.cfg.url,
            RECT { left: card.left + st.s(110), top: card.top + st.s(12), right: card.right - c_pad, bottom: card.top + st.s(32) },
            rgb(238, 240, 244),
            st.font_mono,
            DT_RIGHT.0 | DT_VCENTER.0 | DT_SINGLELINE.0 | DT_END_ELLIPSIS.0,
        );

        let div_brush = CreateSolidBrush(rgb(36, 40, 49));
        let div_rect = RECT {
            left: card.left + c_pad,
            top: card.top + st.s(42),
            right: card.right - c_pad,
            bottom: card.top + st.s(43),
        };
        let _ = FillRect(hdc, &div_rect, div_brush);
        let _ = DeleteObject(div_brush);

        draw_text(
            hdc,
            "Active Show",
            RECT { left: card.left + c_pad, top: card.top + st.s(50), right: card.left + st.s(110), bottom: card.top + st.s(70) },
            rgb(132, 139, 152),
            st.font_label,
            DT_LEFT.0 | DT_VCENTER.0 | DT_SINGLELINE.0,
        );
        draw_text(
            hdc,
            &summary.show_title,
            RECT { left: card.left + st.s(110), top: card.top + st.s(50), right: card.right - c_pad, bottom: card.top + st.s(70) },
            rgb(238, 240, 244),
            st.font_label,
            DT_RIGHT.0 | DT_VCENTER.0 | DT_SINGLELINE.0 | DT_END_ELLIPSIS.0,
        );

        draw_text(
            hdc,
            "Streams",
            RECT { left: card.left + c_pad, top: card.top + st.s(76), right: card.left + st.s(110), bottom: card.top + st.s(96) },
            rgb(132, 139, 152),
            st.font_label,
            DT_LEFT.0 | DT_VCENTER.0 | DT_SINGLELINE.0,
        );
        let feeds_text = if summary.total_channels == 0 {
            "No feeds configured yet".to_string()
        } else {
            format!("{} / {} active", summary.active_channels, summary.total_channels)
        };
        draw_text(
            hdc,
            &feeds_text,
            RECT { left: card.left + st.s(110), top: card.top + st.s(76), right: card.right - c_pad, bottom: card.top + st.s(96) },
            rgb(162, 168, 180),
            st.font_mono,
            DT_RIGHT.0 | DT_VCENTER.0 | DT_SINGLELINE.0,
        );

        let rects = st.button_rects();
        let labels = ["Open Web UI", "Hide to Tray", "Stop Server"];
        for (i, &r) in rects.iter().enumerate() {
            let hovered = st.hover_btn.get() == Some(i);
            let pressed = st.pressed_btn.get() == Some(i);
            let focused = st.focused_btn.get() == Some(i);
            let (fill, border, text_col) = match i {
                BTN_OPEN_WEB => (
                    if pressed { rgb(212, 216, 224) } else if hovered { rgb(255, 255, 255) } else { rgb(238, 240, 244) },
                    if focused { rgb(59, 130, 246) } else { rgb(238, 240, 244) },
                    rgb(14, 16, 19),
                ),
                BTN_HIDE_TRAY => (
                    if pressed { rgb(28, 32, 39) } else if hovered { rgb(42, 48, 59) } else { rgb(33, 37, 44) },
                    if focused { rgb(59, 130, 246) } else { rgb(56, 62, 74) },
                    rgb(238, 240, 244),
                ),
                _ => (
                    if pressed { rgb(44, 20, 24) } else if hovered { rgb(72, 28, 33) } else { rgb(54, 23, 27) },
                    if focused { rgb(239, 68, 68) } else { rgb(125, 45, 52) },
                    rgb(252, 165, 165),
                ),
            };
            draw_rounded_box(hdc, r, st.s(10), fill, border);
            draw_text(
                hdc,
                labels[i],
                r,
                text_col,
                st.font_btn,
                DT_CENTER.0 | DT_VCENTER.0 | DT_SINGLELINE.0,
            );
        }

        draw_text(
            hdc,
            "Closing this window stops all monitors. Use Hide to Tray to keep running.",
            RECT { left: pad, top: st.s(252), right: w - pad, bottom: st.s(274) },
            rgb(112, 119, 132),
            st.font_caption,
            DT_CENTER.0 | DT_VCENTER.0 | DT_SINGLELINE.0 | DT_END_ELLIPSIS.0,
        );

        let _ = BitBlt(hdc_win, 0, 0, w, h, hdc, 0, 0, SRCCOPY);
        SelectObject(hdc, old_bmp);
        let _ = DeleteObject(bmp);
        let _ = DeleteDC(hdc);
        let _ = EndPaint(hwnd, &ps);
    }
}
