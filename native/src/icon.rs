//! 灵动岛的图标，按要的尺寸现画，不带图片资源。只算像素，方便测试。
//!
//! - 任务栏通知区：一颗深色胶囊，有没看过的 AI 消息或等你批准时右上角加个橙点。
//! - 应用图标（桌面快捷方式用）：浅色圆角"屏幕"，上方正中一颗黑色胶囊，就是岛在屏幕上的样子。
//!   存成 .ico 文件（多个尺寸）。

/// 胶囊填充、描边（深浅任务栏上都看得见）、提醒点的颜色。
const FILL: [u8; 3] = [0x1C, 0x1C, 0x1E];
const STROKE: [u8; 3] = [0xE8, 0xE8, 0xEA];
const DOT: [u8; 3] = [0xFF, 0x9F, 0x0A];

/// 圆角矩形的有符号距离（里面为负）。
fn rounded_box(px: f32, py: f32, (cx, cy): (f32, f32), (hw, hh): (f32, f32), r: f32) -> f32 {
    let qx = (px - cx).abs() - (hw - r);
    let qy = (py - cy).abs() - (hh - r);
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    outside + qx.max(qy).min(0.0) - r
}

/// 距离 → 覆盖率（1 像素宽的抗锯齿边）。
fn coverage(d: f32) -> f32 {
    (0.5 - d).clamp(0.0, 1.0)
}

/// 把一层颜色叠到下面（不预乘的 RGBA）。
fn over(dst: &mut [f32; 4], rgb: [u8; 3], a: f32) {
    if a <= 0.0 {
        return;
    }
    let out = a + dst[3] * (1.0 - a);
    for (i, c) in rgb.iter().enumerate() {
        let c = *c as f32 / 255.0;
        dst[i] = (c * a + dst[i] * dst[3] * (1.0 - a)) / out;
    }
    dst[3] = out;
}

/// `size`×`size` 的图标像素，BGRA、不预乘、从上到下。
pub fn pixels(size: u32, dot: bool) -> Vec<u8> {
    let s = size as f32;
    let center = (s / 2.0, s / 2.0);
    let half = (s * 0.46, s * 0.21);
    let radius = half.1;
    let stroke = (s / 12.0).max(1.0);
    let dot_r = s * 0.2;
    let dot_c = (s - dot_r - s * 0.02, dot_r + s * 0.06);
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let mut c = [0.0f32; 4];
            let d = rounded_box(px, py, center, half, radius);
            // 先画描边（整颗胶囊），再在里面画填充
            over(&mut c, STROKE, coverage(d));
            over(&mut c, FILL, coverage(d + stroke));
            if dot {
                let dd = ((px - dot_c.0).powi(2) + (py - dot_c.1).powi(2)).sqrt();
                // 点外面留一圈透明，和胶囊分开
                let gap = coverage(dd - dot_r - stroke);
                c[3] *= 1.0 - gap;
                over(&mut c, DOT, coverage(dd - dot_r));
            }
            let to = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            out.extend([to(c[2]), to(c[1]), to(c[0]), to(c[3])]);
        }
    }
    out
}

/// 应用图标的"屏幕"底色、边框。
const TILE: [u8; 3] = [0xF5, 0xF5, 0xF7];
const TILE_EDGE: [u8; 3] = [0xC7, 0xC7, 0xCC];

/// 应用图标：`size`×`size`，BGRA、不预乘、从上到下。
pub fn app_pixels(size: u32) -> Vec<u8> {
    let s = size as f32;
    let tile = ((s / 2.0, s / 2.0), (s * 0.44, s * 0.44), s * 0.2);
    let edge = (s / 48.0).max(1.0);
    let pill = ((s / 2.0, s * 0.3), (s * 0.24, s * 0.075), s * 0.075);
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let mut c = [0.0f32; 4];
            let d = rounded_box(px, py, tile.0, tile.1, tile.2);
            over(&mut c, TILE_EDGE, coverage(d));
            over(&mut c, TILE, coverage(d + edge));
            over(
                &mut c,
                FILL,
                coverage(rounded_box(px, py, pill.0, pill.1, pill.2)),
            );
            let to = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            out.extend([to(c[2]), to(c[1]), to(c[0]), to(c[3])]);
        }
    }
    out
}

/// 应用图标放进 .ico 的尺寸。
pub const ICO_SIZES: [u32; 6] = [16, 24, 32, 48, 64, 256];

/// 把几张 BGRA 图（从上到下）打包成 .ico 文件（32 位 BMP 格式，带透明度）。
pub fn ico(images: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    // ICONDIR
    out.extend(0u16.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend((images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len();
    let mut blobs = Vec::new();
    for (size, px) in images {
        let s = *size as usize;
        let mask_row = s.div_ceil(32) * 4;
        let mut b = Vec::with_capacity(40 + px.len() + mask_row * s);
        // BITMAPINFOHEADER：高度是两倍（颜色 + 掩码）
        b.extend(40u32.to_le_bytes());
        b.extend((s as i32).to_le_bytes());
        b.extend((2 * s as i32).to_le_bytes());
        b.extend(1u16.to_le_bytes());
        b.extend(32u16.to_le_bytes());
        b.extend([0u8; 24]);
        // 颜色：从下到上
        for row in (0..s).rev() {
            b.extend(&px[row * s * 4..(row + 1) * s * 4]);
        }
        // 掩码全 0（透明度看 alpha）
        b.extend(std::iter::repeat_n(0u8, mask_row * s));
        // ICONDIRENTRY：256 写成 0
        let dim = if s >= 256 { 0 } else { s as u8 };
        out.extend([dim, dim, 0, 0]);
        out.extend(1u16.to_le_bytes());
        out.extend(32u16.to_le_bytes());
        out.extend((b.len() as u32).to_le_bytes());
        out.extend((offset as u32).to_le_bytes());
        offset += b.len();
        blobs.push(b);
    }
    for b in blobs {
        out.extend(b);
    }
    out
}

/// 应用图标的 .ico 文件内容。
pub fn app_ico() -> Vec<u8> {
    let images: Vec<(u32, Vec<u8>)> = ICO_SIZES.iter().map(|&s| (s, app_pixels(s))).collect();
    ico(&images)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(px: &[u8], size: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * size + x) * 4) as usize;
        [px[i], px[i + 1], px[i + 2], px[i + 3]]
    }

    #[test]
    fn draws_a_capsule_with_an_optional_dot() {
        for size in [16, 20, 24, 32] {
            let plain = pixels(size, false);
            assert_eq!(plain.len(), (size * size * 4) as usize);
            // 正中是深色胶囊，角上透明
            assert_eq!(
                at(&plain, size, size / 2, size / 2),
                [0x1E, 0x1C, 0x1C, 255]
            );
            assert_eq!(at(&plain, size, 0, 0)[3], 0);
            assert_eq!(at(&plain, size, size / 2, 0)[3], 0);
            // 右上角：没有点时透明，有点时是橙色
            let dotted = pixels(size, true);
            let (dx, dy) = ((size as f32 * 0.78) as u32, (size as f32 * 0.12) as u32);
            assert_eq!(at(&plain, size, dx, dy)[3], 0);
            assert_eq!(at(&dotted, size, dx, dy), [0x0A, 0x9F, 0xFF, 255]);
        }
    }

    #[test]
    fn app_icon_is_a_screen_with_the_island() {
        let size = 64;
        let px = app_pixels(size);
        // 屏幕正中是浅色，上方正中是黑色胶囊，角上透明
        assert_eq!(at(&px, size, 32, 44), [0xF7, 0xF5, 0xF5, 255]);
        assert_eq!(at(&px, size, 32, 19), [0x1E, 0x1C, 0x1C, 255]);
        assert_eq!(at(&px, size, 0, 0)[3], 0);
    }

    #[test]
    fn ico_file_layout() {
        let f = app_ico();
        let u16_at = |i: usize| u16::from_le_bytes([f[i], f[i + 1]]);
        let u32_at = |i: usize| u32::from_le_bytes([f[i], f[i + 1], f[i + 2], f[i + 3]]);
        assert_eq!(
            (u16_at(0), u16_at(2), u16_at(4)),
            (0, 1, ICO_SIZES.len() as u16)
        );
        let mut end = 6 + 16 * ICO_SIZES.len();
        for (k, &s) in ICO_SIZES.iter().enumerate() {
            let e = 6 + 16 * k;
            assert_eq!(f[e] as u32, if s == 256 { 0 } else { s });
            let (len, off) = (u32_at(e + 8) as usize, u32_at(e + 12) as usize);
            assert_eq!(off, end);
            let mask = (s as usize).div_ceil(32) * 4 * s as usize;
            assert_eq!(len, 40 + (s * s * 4) as usize + mask);
            assert_eq!(u32_at(off), 40);
            assert_eq!(u32_at(off + 8), 2 * s);
            end += len;
        }
        assert_eq!(end, f.len());
    }
}
