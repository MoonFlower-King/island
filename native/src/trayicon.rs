//! 任务栏通知区的小图标：一颗深色胶囊（灵动岛），有没看过的 AI 消息或等你批准时右上角加个橙点。
//! 按系统要的尺寸现画，不带图片资源。只算像素，方便测试。

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
}
