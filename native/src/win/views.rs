//! 内容绘制：音乐胶囊和展开面板。只在内容变化时重画（换歌、整分钟、播放中每秒一次、鼠标移到按钮上）。

use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD,
};
use windows::core::{HSTRING, Result};

use island::color::{Palette, Rgb};
use island::metrics::{COMPACT, EXPANDED, Rect};
use island::text::{clock, format_duration, weekday_zh};

use super::gfx::{Align, Painter, TextStyle, color};
use super::media::Snapshot;

/// 展开面板里可以点的东西。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    TabMusic,
    Pin,
    Previous,
    PlayPause,
    Next,
}

/// 展开面板的布局（相对胶囊左上角，DIP）。
pub mod layout {
    use island::metrics::Rect;

    pub const TAB_MUSIC: Rect = Rect::new(16.0, 12.0, 36.0, 28.0);
    pub const CLOCK: Rect = Rect::new(372.0, 12.0, 52.0, 28.0);
    pub const WEEKDAY: Rect = Rect::new(334.0, 12.0, 36.0, 28.0);
    pub const PIN: Rect = Rect::new(304.0, 13.0, 26.0, 26.0);

    pub const ART: Rect = Rect::new(16.0, 56.0, 120.0, 120.0);
    pub const TITLE: Rect = Rect::new(152.0, 60.0, 272.0, 26.0);
    pub const ARTIST: Rect = Rect::new(152.0, 88.0, 272.0, 20.0);
    pub const PREVIOUS: Rect = Rect::new(152.0, 128.0, 40.0, 40.0);
    pub const PLAY: Rect = Rect::new(200.0, 124.0, 48.0, 48.0);
    pub const NEXT: Rect = Rect::new(256.0, 128.0, 40.0, 40.0);
    pub const PROGRESS: Rect = Rect::new(16.0, 196.0, 408.0, 4.0);
    pub const TIME_LEFT: Rect = Rect::new(16.0, 204.0, 80.0, 18.0);
    pub const TIME_RIGHT: Rect = Rect::new(344.0, 204.0, 80.0, 18.0);
}

/// 鼠标位置（相对胶囊）落在哪个按钮上。没有音乐时播放按钮不可点。
pub fn hit_test(x: f32, y: f32, has_media: bool) -> Option<Hit> {
    use layout::*;
    let targets: [(Rect, Hit, bool); 5] = [
        (TAB_MUSIC, Hit::TabMusic, true),
        (PIN, Hit::Pin, true),
        (PREVIOUS, Hit::Previous, has_media),
        (PLAY, Hit::PlayPause, has_media),
        (NEXT, Hit::Next, has_media),
    ];
    targets
        .iter()
        .find(|(r, _, enabled)| *enabled && r.inflate(2.0).contains(x, y))
        .map(|(_, h, _)| *h)
}

pub struct ViewState<'a> {
    pub palette: &'a Palette,
    pub accent: Rgb,
    pub media: Option<&'a Snapshot>,
    pub now_ms: u64,
    pub hour: u16,
    pub minute: u16,
    pub weekday: u16,
    pub pinned: bool,
    pub hovered: Option<Hit>,
    pub pressed: Option<Hit>,
}

mod glyph {
    pub const MUSIC: char = '\u{E8D6}';
    pub const PIN: char = '\u{E718}';
    pub const UNPIN: char = '\u{E77A}';
    pub const PLAY: char = '\u{E768}';
    pub const PAUSE: char = '\u{E769}';
    pub const PREVIOUS: char = '\u{E892}';
    pub const NEXT: char = '\u{E893}';
}

pub fn draw_compact(p: &Painter, v: &ViewState) -> Result<()> {
    let fg = v.palette.foreground;
    let Some(m) = v.media else { return Ok(()) };
    let art = Rect::new(7.0, 6.0, 26.0, 26.0);
    draw_art(p, m, art, 7.0, v)?;
    let family = p.gfx.text_font.clone();
    let style = TextStyle {
        family: &family,
        size: 12.5,
        weight: DWRITE_FONT_WEIGHT_SEMI_BOLD,
        align: Align::Leading,
    };
    let title = if m.title.is_empty() {
        "正在播放"
    } else {
        &m.title
    };
    p.text(
        title,
        Rect::new(42.0, 0.0, 170.0, COMPACT.height),
        &style,
        color(fg, 0.95),
    )?;
    Ok(())
}

pub fn draw_expanded(p: &Painter, v: &ViewState) -> Result<()> {
    use layout::*;
    let fg = v.palette.foreground;
    let family = p.gfx.text_font.clone();

    // 标题栏：音乐标签、置顶、星期、时间
    p.fill_round(TAB_MUSIC, 14.0, color(fg, 0.20))?;
    p.icon(glyph::MUSIC, TAB_MUSIC, 14.0, color(fg, 1.0))?;

    if v.hovered == Some(Hit::Pin) {
        p.fill_circle(PIN.center_x(), PIN.center_y(), 13.0, color(fg, 0.12))?;
    }
    let pin_color = if v.pinned {
        color(v.accent, 1.0)
    } else {
        color(fg, 0.6)
    };
    p.icon(
        if v.pinned { glyph::UNPIN } else { glyph::PIN },
        PIN,
        12.0,
        pin_color,
    )?;

    let small = TextStyle {
        family: &family,
        size: 11.0,
        weight: DWRITE_FONT_WEIGHT_NORMAL,
        align: Align::Trailing,
    };
    p.text(weekday_zh(v.weekday), WEEKDAY, &small, color(fg, 0.4))?;
    let clock_style = TextStyle {
        family: &family,
        size: 14.0,
        weight: DWRITE_FONT_WEIGHT_SEMI_BOLD,
        align: Align::Trailing,
    };
    p.text(
        &clock(v.hour, v.minute),
        CLOCK,
        &clock_style,
        color(fg, 1.0),
    )?;

    match v.media {
        Some(m) => draw_player(p, m, v, &family),
        None => draw_empty(p, v, &family),
    }
}

fn draw_player(p: &Painter, m: &Snapshot, v: &ViewState, family: &HSTRING) -> Result<()> {
    use layout::*;
    let fg = v.palette.foreground;
    draw_art(p, m, ART, 14.0, v)?;

    let title = TextStyle {
        family,
        size: 16.0,
        weight: DWRITE_FONT_WEIGHT_SEMI_BOLD,
        align: Align::Leading,
    };
    p.text(
        if m.title.is_empty() {
            "未知歌曲"
        } else {
            &m.title
        },
        TITLE,
        &title,
        color(fg, 1.0),
    )?;
    let artist = TextStyle {
        family,
        size: 13.0,
        weight: DWRITE_FONT_WEIGHT_NORMAL,
        align: Align::Leading,
    };
    p.text(&m.artist, ARTIST, &artist, color(fg, 0.6))?;

    for (rect, hit, g, size, enabled) in [
        (
            PREVIOUS,
            Hit::Previous,
            glyph::PREVIOUS,
            16.0,
            m.can_previous,
        ),
        (
            PLAY,
            Hit::PlayPause,
            if m.playing { glyph::PAUSE } else { glyph::PLAY },
            20.0,
            true,
        ),
        (NEXT, Hit::Next, glyph::NEXT, 16.0, m.can_next),
    ] {
        let radius = rect.w / 2.0;
        let pressed = v.pressed == Some(hit);
        if hit == Hit::PlayPause {
            p.fill_circle(
                rect.center_x(),
                rect.center_y(),
                radius,
                color(fg, if pressed { 0.14 } else { 0.10 }),
            )?;
        } else if v.hovered == Some(hit) {
            p.fill_circle(
                rect.center_x(),
                rect.center_y(),
                radius,
                color(fg, if pressed { 0.16 } else { 0.10 }),
            )?;
        }
        if v.hovered == Some(hit) && hit == Hit::PlayPause {
            p.fill_circle(rect.center_x(), rect.center_y(), radius, color(fg, 0.06))?;
        }
        p.icon(g, rect, size, color(fg, if enabled { 1.0 } else { 0.3 }))?;
    }

    // 进度
    if m.duration > 0.0 {
        let position = m.position_at(v.now_ms);
        let fraction = (position / m.duration).clamp(0.0, 1.0) as f32;
        p.fill_round(PROGRESS, 2.0, color(fg, 0.15))?;
        if fraction > 0.0 {
            let filled = Rect::new(
                PROGRESS.x,
                PROGRESS.y,
                (PROGRESS.w * fraction).max(4.0),
                PROGRESS.h,
            );
            p.fill_round(filled, 2.0, color(v.accent, 1.0))?;
        }
        let left = TextStyle {
            family,
            size: 11.0,
            weight: DWRITE_FONT_WEIGHT_NORMAL,
            align: Align::Leading,
        };
        let right = TextStyle {
            align: Align::Trailing,
            ..left
        };
        p.text(&format_duration(position), TIME_LEFT, &left, color(fg, 0.5))?;
        p.text(
            &format_duration(m.duration),
            TIME_RIGHT,
            &right,
            color(fg, 0.5),
        )?;
    }
    Ok(())
}

fn draw_empty(p: &Painter, v: &ViewState, family: &HSTRING) -> Result<()> {
    let fg = v.palette.foreground;
    let w = EXPANDED.width;
    p.icon(
        glyph::MUSIC,
        Rect::new(0.0, 92.0, w, 36.0),
        28.0,
        color(fg, 0.3),
    )?;
    let main = TextStyle {
        family,
        size: 13.0,
        weight: DWRITE_FONT_WEIGHT_NORMAL,
        align: Align::Center,
    };
    p.text(
        "没有正在播放的音乐",
        Rect::new(0.0, 134.0, w, 22.0),
        &main,
        color(fg, 0.6),
    )?;
    let hint = TextStyle {
        family,
        size: 11.0,
        ..main
    };
    p.text(
        "在 Spotify、网易云音乐、QQ 音乐或浏览器里播放后会显示在这里",
        Rect::new(16.0, 158.0, w - 32.0, 18.0),
        &hint,
        color(fg, 0.4),
    )
}

fn draw_art(p: &Painter, m: &Snapshot, r: Rect, radius: f32, v: &ViewState) -> Result<()> {
    match &m.art {
        Some(art) => p.image(&art.pixels, art.width, art.height, r, radius),
        None => {
            p.fill_round(r, radius, color(v.palette.foreground, 0.12))?;
            p.icon(
                glyph::MUSIC,
                r,
                r.w * 0.42,
                color(v.palette.foreground, 0.5),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buttons_do_not_overlap_and_fit_in_the_panel() {
        use layout::*;
        let all = [
            TAB_MUSIC, PIN, WEEKDAY, CLOCK, ART, TITLE, ARTIST, PREVIOUS, PLAY, NEXT, PROGRESS,
            TIME_LEFT, TIME_RIGHT,
        ];
        for r in all {
            assert!(
                r.x >= 16.0 - 0.01 && r.right() <= EXPANDED.width - 16.0 + 0.01,
                "{r:?}"
            );
            assert!(r.bottom() <= EXPANDED.height - 14.0, "{r:?}");
        }
        let buttons = [TAB_MUSIC, PIN, PREVIOUS, PLAY, NEXT];
        for (i, a) in buttons.iter().enumerate() {
            for b in &buttons[i + 1..] {
                let overlap =
                    a.x < b.right() && b.x < a.right() && a.y < b.bottom() && b.y < a.bottom();
                assert!(!overlap, "{a:?} {b:?}");
            }
        }
    }

    #[test]
    fn hit_testing() {
        use layout::*;
        assert_eq!(
            hit_test(PLAY.center_x(), PLAY.center_y(), true),
            Some(Hit::PlayPause)
        );
        assert_eq!(hit_test(PLAY.center_x(), PLAY.center_y(), false), None);
        assert_eq!(
            hit_test(PIN.center_x(), PIN.center_y(), false),
            Some(Hit::Pin)
        );
        assert_eq!(hit_test(5.0, 250.0, true), None);
    }
}
