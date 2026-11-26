use std::mem::size_of;
use windows::Win32::Foundation::BOOL;
use windows::Win32::Graphics::Gdi::{
    CreateBitmap, CreateDIBSection, DeleteObject, GetDC, ReleaseDC, BITMAPINFO, BITMAPINFOHEADER,
    BI_RGB, DIB_RGB_COLORS,
};
use windows::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, HICON, ICONINFO};

pub(super) fn dist_point_to_seg(
    px: f32,
    py: f32,
    ax: f32,
    ay: f32,
    bx: f32,
    by: f32,
) -> f32 {
    let vx = bx - ax;
    let vy = by - ay;
    let wx = px - ax;
    let wy = py - ay;
    let len_sq = vx * vx + vy * vy;
    if len_sq <= 1e-6 {
        return (wx * wx + wy * wy).sqrt();
    }
    let t = ((wx * vx + wy * vy) / len_sq).clamp(0.0, 1.0);
    let dx = px - (ax + t * vx);
    let dy = py - (ay + t * vy);
    (dx * dx + dy * dy).sqrt()
}

fn blend(dst: [u8; 4], src: [u8; 4]) -> [u8; 4] {
    let sa = src[3] as f32 / 255.0;
    let da = dst[3] as f32 / 255.0;
    let out_a = sa + da * (1.0 - sa);
    if out_a <= 1e-5 {
        return [0, 0, 0, 0];
    }
    let mix = |d: u8, s: u8| -> u8 {
        (((s as f32 * sa) + (d as f32 * da * (1.0 - sa))) / out_a)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    [
        mix(dst[0], src[0]),
        mix(dst[1], src[1]),
        mix(dst[2], src[2]),
        (out_a * 255.0).round().clamp(0.0, 255.0) as u8,
    ]
}

fn coverage_px(d: f32, aa: f32) -> u8 {
    ((0.5 - d / aa).clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Rasterizes PGMWatch's Logo.jpg vector mark (rounded dark frame, open white bezel,
/// emerald 3-peak signal pulse, coral live tally dot) as anti-aliased BGRA pixels.
pub(super) fn render_logo_bgra(size: i32, bg_matte: Option<[u8; 3]>) -> Vec<u8> {
    let s = size.max(16) as usize;
    let mut out = vec![0u8; s * s * 4];
    let scale = 32.0 / s as f32;
    let aa = scale * 1.15;

    let wave_pts: [(f32, f32); 9] = [
        (8.1, 16.2),
        (10.5, 16.2),
        (11.7, 13.7),
        (13.3, 18.4),
        (15.3, 11.1),
        (17.1, 21.1),
        (18.8, 14.1),
        (20.1, 16.2),
        (23.7, 16.2),
    ];

    let frame_segs: [(f32, f32, f32, f32); 4] = [
        (20.2, 7.4, 7.2, 7.4),
        (4.8, 9.8, 4.8, 22.0),
        (7.2, 24.4, 24.4, 24.4),
        (26.8, 22.0, 26.8, 14.8),
    ];

    for py in 0..s {
        for px in 0..s {
            let x = (px as f32 + 0.5) * scale;
            let y = (py as f32 + 0.5) * scale;

            let qx = (x - 16.0).abs() - (15.5 - 7.2);
            let qy = (y - 16.0).abs() - (15.5 - 7.2);
            let tile_sdf = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt()
                + qx.max(qy).min(0.0)
                - 7.2;
            let tile_a = coverage_px(tile_sdf, aa);
            let mut col: [u8; 4] = if let Some([mr, mg, mb]) = bg_matte {
                let base = [mb, mg, mr, 255];
                blend(base, [33, 24, 21, tile_a])
            } else {
                [33, 24, 21, tile_a]
            };

            let mut frame_dist = 999.0f32;
            for &(ax, ay, bx, by) in &frame_segs {
                frame_dist = frame_dist.min(dist_point_to_seg(x, y, ax, ay, bx, by));
            }
            let corners: [(f32, f32, f32, f32); 3] = [
                (7.2, 9.8, -1.0, -1.0),
                (7.2, 22.0, -1.0, 1.0),
                (24.4, 22.0, 1.0, 1.0),
            ];
            for &(cx, cy, sx, sy) in &corners {
                if (x - cx) * sx >= 0.0 && (y - cy) * sy >= 0.0 {
                    let d_arc = (((x - cx).powi(2) + (y - cy).powi(2)).sqrt() - 2.4).abs();
                    frame_dist = frame_dist.min(d_arc);
                }
            }
            let bezel_a = coverage_px(frame_dist - 1.25, aa);
            if bezel_a > 0 {
                col = blend(col, [248, 245, 243, bezel_a]);
            }

            let mut wave_dist = 999.0f32;
            for w in wave_pts.windows(2) {
                wave_dist = wave_dist.min(dist_point_to_seg(x, y, w[0].0, w[0].1, w[1].0, w[1].1));
            }
            let wave_a = coverage_px(wave_dist - 1.18, aa);
            if wave_a > 0 {
                col = blend(col, [101, 201, 21, wave_a]);
            }

            let dot_dist = ((x - 25.5).powi(2) + (y - 8.5).powi(2)).sqrt() - 2.7;
            let dot_a = coverage_px(dot_dist, aa);
            if dot_a > 0 {
                col = blend(col, [48, 61, 240, dot_a]);
            }

            let idx = (py * s + px) * 4;
            out[idx..idx + 4].copy_from_slice(&col);
        }
    }
    out
}

pub(super) fn create_logo_hicon(size: i32) -> HICON {
    let pixels = render_logo_bgra(size, None);
    let bi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: size,
            biHeight: -size,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    unsafe {
        let mut ptr = std::ptr::null_mut();
        let hdc_screen = GetDC(None);
        let hbm_color = CreateDIBSection(hdc_screen, &bi, DIB_RGB_COLORS, &mut ptr, None, 0);
        ReleaseDC(None, hdc_screen);
        let Ok(hbm_color) = hbm_color else {
            return HICON::default();
        };
        if !ptr.is_null() {
            std::ptr::copy_nonoverlapping(pixels.as_ptr(), ptr as *mut u8, pixels.len());
        }
        let hbm_mask = CreateBitmap(size, size, 1, 1, None);
        let info = ICONINFO {
            fIcon: BOOL(1),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: hbm_mask,
            hbmColor: hbm_color,
        };
        let icon = CreateIconIndirect(&info).unwrap_or_default();
        let _ = DeleteObject(hbm_color);
        let _ = DeleteObject(hbm_mask);
        icon
    }
}
