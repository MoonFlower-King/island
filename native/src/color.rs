//! 颜色计算：封面取色、对比度。和 WPF 版的 ColorExtractor / ColorMath 一致。

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// 0..1 的分量，给 Direct2D / Composition 用。
    pub fn to_f32(self) -> [f32; 3] {
        [
            self.r as f32 / 255.0,
            self.g as f32 / 255.0,
            self.b as f32 / 255.0,
        ]
    }
}

/// 主题配色。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub background: Rgb,
    pub foreground: Rgb,
    pub accent: Rgb,
}

pub const DARK: Palette = Palette {
    background: Rgb::new(0x00, 0x00, 0x00),
    foreground: Rgb::new(0xFF, 0xFF, 0xFF),
    accent: Rgb::new(0x0A, 0x84, 0xFF),
};

pub const LIGHT: Palette = Palette {
    background: Rgb::new(0xF7, 0xF7, 0xF8),
    foreground: Rgb::new(0x1C, 0x1C, 0x1E),
    accent: Rgb::new(0x00, 0x7A, 0xFF),
};

/// WCAG 相对亮度。
pub fn luminance(c: Rgb) -> f64 {
    fn channel(v: u8) -> f64 {
        let s = v as f64 / 255.0;
        if s <= 0.03928 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    }
    0.2126 * channel(c.r) + 0.7152 * channel(c.g) + 0.0722 * channel(c.b)
}

/// WCAG 对比度，1..21。
pub fn contrast(a: Rgb, b: Rgb) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

fn mix(a: Rgb, b: Rgb, t: f64) -> Rgb {
    let m = |x: u8, y: u8| {
        (x as f64 + (y as f64 - x as f64) * t)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Rgb::new(m(a.r, b.r), m(a.g, b.g), m(a.b, b.b))
}

/// 把颜色往白或往黑调，直到在背景上的对比度至少为 `min`。
pub fn ensure_readable(color: Rgb, background: Rgb, min: f64) -> Rgb {
    if contrast(color, background) >= min {
        return color;
    }
    let target = if luminance(background) > 0.45 {
        Rgb::new(0, 0, 0)
    } else {
        Rgb::new(255, 255, 255)
    };
    let mut t: f64 = 0.0;
    let mut c = color;
    while t < 1.0 && contrast(c, background) < min {
        t += 0.05;
        c = mix(color, target, t.min(1.0));
    }
    c
}

/// 从封面像素（BGRA，预乘或不预乘都行）里挑一个有代表性的鲜艳颜色：
/// 按饱和度和亮度加权平均，太暗、太亮、太灰的像素权重很低。
pub fn dominant(bgra: &[u8]) -> Option<Rgb> {
    let (mut r, mut g, mut b, mut total) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    for px in bgra.chunks_exact(4).step_by(3) {
        let (pb, pg, pr, pa) = (px[0] as f64, px[1] as f64, px[2] as f64, px[3] as f64);
        if pa < 128.0 {
            continue;
        }
        let max = pr.max(pg).max(pb);
        let min = pr.min(pg).min(pb);
        let saturation = if max > 0.0 { (max - min) / max } else { 0.0 };
        let value = max / 255.0;
        let weight = saturation * saturation * (1.0 - (value - 0.65).abs()).max(0.05) + 0.002;
        r += pr * weight;
        g += pg * weight;
        b += pb * weight;
        total += weight;
    }
    if total <= 0.0 {
        return None;
    }
    Some(Rgb::new(
        (r / total).round() as u8,
        (g / total).round() as u8,
        (b / total).round() as u8,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contrast_extremes() {
        assert!((contrast(Rgb::new(0, 0, 0), Rgb::new(255, 255, 255)) - 21.0).abs() < 0.01);
        assert!((contrast(DARK.accent, DARK.accent) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn makes_dark_colors_readable_on_black() {
        let navy = Rgb::new(0x10, 0x10, 0x40);
        let fixed = ensure_readable(navy, DARK.background, 3.0);
        assert!(contrast(fixed, DARK.background) >= 3.0);
        assert_eq!(
            ensure_readable(DARK.accent, DARK.background, 3.0),
            DARK.accent
        );
        let pale = ensure_readable(Rgb::new(0xFF, 0xF0, 0xC0), LIGHT.background, 3.0);
        assert!(contrast(pale, LIGHT.background) >= 3.0);
    }

    #[test]
    fn picks_the_vivid_color() {
        // 大半灰色 + 一块红色：应该选出偏红的颜色
        let mut px = Vec::new();
        for i in 0..300 {
            if i % 3 == 0 {
                px.extend_from_slice(&[30, 30, 220, 255])
            } else {
                px.extend_from_slice(&[128, 128, 128, 255])
            }
        }
        let c = dominant(&px).unwrap();
        assert!(c.r > c.g + 40 && c.r > c.b + 40, "{c:?}");
        assert_eq!(dominant(&[0, 0, 0, 0]), None);
    }
}
