//! 内容绘制：音乐胶囊、音量提示和展开面板的各个标签。
//!
//! 只在内容变化时重画（换歌、整分钟、播放中每秒一次、鼠标移到按钮上）。
//! 画的同时登记可点击的区域，布局和点击判断用的是同一份坐标，不会对不上。

use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FONT_WEIGHT, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD,
};
use windows::core::{HSTRING, Result};

use island::clip::Entry;
use island::color::{self, ACCENT_PRESETS, BACKGROUND_PRESETS, FOREGROUND_PRESETS, Palette, Rgb};
use island::edit::TextEdit;
use island::level;
use island::metrics::{
    COMPACT, EXPANDED, Edge, IDLE_CLOCK, LEVEL, NOTIFICATION, Rect, SIDE_CLOCK, SIDE_COMPACT,
    SIDE_LEVEL, SIDE_NOTIFICATION,
};
use island::settings::{Settings, ThemeMode};
use island::shelf::Item as ShelfEntry;
use island::text::{clock, format_duration, speed, weekday_zh};
use island::todo::Todo;

use super::gfx::{Align, FieldDraw, FieldGeometry, Painter, TextStyle, color};
use super::media::{Art, Snapshot};
use super::system::Sample;
use super::translator::{Status, TranslateUi};
use island::translate::{self as tr, Format, PRESETS};

/// 展开面板的功能标签。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Music,
    Todo,
    Clipboard,
    Shelf,
    Translate,
    Monitor,
    Appearance,
    /// AI 动态：Claude、Codex 等完成任务或等你确认的消息。
    Ai,
}

impl Tab {
    pub const ALL: [Tab; 8] = [
        Tab::Music,
        Tab::Todo,
        Tab::Clipboard,
        Tab::Shelf,
        Tab::Translate,
        Tab::Monitor,
        Tab::Appearance,
        Tab::Ai,
    ];

    /// 和 WPF 版 settings.json 里 LastTab 的取值一致。
    pub fn name(self) -> &'static str {
        match self {
            Tab::Music => "Music",
            Tab::Todo => "Todo",
            Tab::Clipboard => "Clipboard",
            Tab::Shelf => "Shelf",
            Tab::Translate => "Translate",
            Tab::Monitor => "Monitor",
            Tab::Appearance => "Appearance",
            Tab::Ai => "Ai",
        }
    }

    pub fn parse(s: &str) -> Tab {
        Tab::ALL
            .into_iter()
            .find(|t| t.name() == s)
            .unwrap_or(Tab::Music)
    }

    fn glyph(self) -> char {
        match self {
            Tab::Music => '\u{E8D6}',
            Tab::Todo => '\u{E762}',
            Tab::Clipboard => '\u{E77F}',
            Tab::Shelf => '\u{E8B7}',
            Tab::Translate => '\u{E8C1}',
            Tab::Monitor => '\u{E9D9}',
            Tab::Appearance => '\u{E790}',
            Tab::Ai => '\u{E99A}',
        }
    }
}

/// 文字输入框。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    /// 添加待办。
    Todo,
    /// 自定义颜色：0 背景 / 1 文字 / 2 强调色。
    Hex(u8),
    /// "按时间"主题：白天 / 夜晚开始的时间。
    DayStarts,
    NightStarts,
    /// 要翻译的文字（多行）。
    TranslateInput,
    /// 翻译服务设置：地址、模型、Key。
    BaseUrl,
    Model,
    ApiKey,
}

impl Field {
    pub fn max_chars(self) -> usize {
        match self {
            Field::Todo => island::todo::MAX_LENGTH,
            Field::Hex(_) => 7,
            Field::DayStarts | Field::NightStarts => 5,
            Field::TranslateInput => 20_000,
            Field::BaseUrl => 500,
            Field::Model | Field::ApiKey => 200,
        }
    }

    pub fn multiline(self) -> bool {
        self == Field::TranslateInput
    }

    pub fn font_size(self) -> f32 {
        match self {
            Field::Todo | Field::TranslateInput => 13.0,
            _ => 12.0,
        }
    }

    /// 只填 ASCII 的框（颜色、时间、地址、Key）不需要输入法。
    pub fn wants_ime(self) -> bool {
        matches!(self, Field::Todo | Field::TranslateInput | Field::Model)
    }

    /// Key 显示成圆点。
    pub fn masked(self) -> bool {
        self == Field::ApiKey
    }
}

/// 输入框里实际显示的文字：Key 换成圆点（一个字符对一个圆点，光标位置不变）。
pub fn display_text(field: Field, text: &str) -> std::borrow::Cow<'_, str> {
    if field.masked() {
        std::borrow::Cow::Owned(
            text.chars()
                .map(|c| if c.len_utf16() == 1 { '•' } else { c })
                .collect(),
        )
    } else {
        std::borrow::Cow::Borrowed(text)
    }
}

/// 正在编辑的输入框：内容、光标、输入法组字。
pub struct Editing<'a> {
    pub field: Field,
    pub edit: &'a TextEdit,
    /// 输入法正在组的字和组字内的光标（UTF-16）。
    pub composition: &'a str,
    pub composition_caret: usize,
}

/// 可以点的东西。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hit {
    Tab(Tab),
    Pin,
    Previous,
    PlayPause,
    Next,
    /// 音量条：按下 / 拖动时按横坐标设置音量。
    Volume,
    /// 进度条：按下 / 拖动时预览位置，松开时跳过去。
    Seek,
    ClipItem(usize),
    ClipDelete(usize),
    ClipClear,
    Theme(ThemeMode),
    /// 自定义颜色：(0 背景 / 1 文字 / 2 强调色, 第几个色块)
    Swatch(u8, u8),
    IdleStyle(bool),
    MediaStyle(bool),
    Dock(Edge),
    /// 换到下一块屏幕。
    NextMonitor,
    Field(Field),
    TodoToggle(usize),
    TodoDelete(usize),
    TodoClearDone,
    /// 中转站：一行（按下拖出、双击打开）、在资源管理器中显示、移出、清空。
    ShelfItem(usize),
    ShelfReveal(usize),
    ShelfRemove(usize),
    ShelfClear,
    /// 翻译（翻译中时是"停止"）。
    TranslateGo,
    TranslatePaste,
    TranslateCopy,
    TranslateSettings,
    /// 设置页：选服务（预设下标）、选格式、返回、测试、保存。
    Provider(u8),
    ApiFormat(Format),
    SettingsBack,
    SettingsTest,
    SettingsSave,
    /// AI 动态：标记一条已读、全部已读、清空。
    AiItem(usize),
    AiReadAll,
    AiClear,
}

/// AI 动态里的一条消息。
#[derive(Clone)]
pub struct AiItem {
    pub title: String,
    pub body: String,
    /// 收到的时间（HH:MM）。
    pub at: String,
    pub read: bool,
    /// 软件自己的图标（桌面版的系统通知才有）。
    pub icon: Option<std::sync::Arc<Art>>,
}

pub const AI_ROWS: usize = 4;
const AI_TOP: f32 = 78.0;
const AI_ROW_HEIGHT: f32 = 42.0;

/// 一次绘制登记下来的可点区域（相对胶囊左上角）。
#[derive(Default, Clone)]
pub struct Targets {
    items: Vec<(Rect, Hit)>,
    fields: Vec<(Field, FieldGeometry)>,
    /// 译文区域：(区域, 实际滚动位置, 最大滚动位置)。
    pub output: Option<(Rect, f32, f32)>,
}

impl Targets {
    /// 某个输入框这次画在哪、文字原点和光标在哪。
    pub fn field(&self, f: Field) -> Option<FieldGeometry> {
        self.fields.iter().find(|(k, _)| *k == f).map(|(_, g)| *g)
    }

    fn add(&mut self, r: Rect, hit: Hit) {
        self.items.push((r, hit));
    }

    pub fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        self.items
            .iter()
            .rev()
            .find(|(r, _)| r.inflate(1.0).contains(x, y))
            .map(|(_, h)| *h)
    }

    pub fn rect(&self, hit: Hit) -> Option<Rect> {
        self.items
            .iter()
            .rev()
            .find(|(_, h)| *h == hit)
            .map(|(r, _)| *r)
    }

    pub fn any(&self, f: impl Fn(Hit) -> bool) -> bool {
        self.items.iter().any(|(_, h)| f(*h))
    }
}

/// 绘制需要的全部状态。
pub struct ViewState<'a> {
    pub palette: &'a Palette,
    pub accent: Rgb,
    pub settings: &'a Settings,
    pub tab: Tab,
    pub media: Option<&'a Snapshot>,
    /// 播放进度（秒）和总时长（不知道时为 None）。
    pub progress: Option<(f64, Option<f64>)>,
    pub volume: Option<(f64, bool)>,
    /// 正在拖进度条：预览的位置（0–1）。
    pub seek: Option<f64>,
    pub clips: &'a [Entry],
    pub clip_scroll: usize,
    pub todos: &'a [Todo],
    pub shelf: &'a [ShelfEntry],
    pub shelf_scroll: usize,
    /// 正拖着文件经过岛。
    pub drag_over: bool,
    pub todo_scroll: usize,
    pub editing: Option<Editing<'a>>,
    pub translate: &'a TranslateUi,
    /// 有多块屏幕时，当前所在屏幕的名字。
    pub monitor_label: Option<String>,
    /// 当前翻译服务的简短说明、是否已经可以翻译。
    pub translate_summary: (String, bool),
    pub today: &'a str,
    pub sample: Option<Sample>,
    pub hour: u16,
    pub minute: u16,
    pub weekday: u16,
    pub pinned: bool,
    pub hovered: Option<Hit>,
    pub pressed: Option<Hit>,
    pub ai: &'a [AiItem],
}

mod glyph {
    pub const PIN: char = '\u{E718}';
    pub const UNPIN: char = '\u{E77A}';
    pub const PLAY: char = '\u{E768}';
    pub const PAUSE: char = '\u{E769}';
    pub const PREVIOUS: char = '\u{E892}';
    pub const NEXT: char = '\u{E893}';
    pub const MUSIC: char = '\u{E8D6}';
    pub const DELETE: char = '\u{E74D}';
    pub const CLOSE: char = '\u{E711}';
    pub const CLIPBOARD: char = '\u{E77F}';
    pub const CHECK: char = '\u{E73E}';
    pub const TODO: char = '\u{E762}';
    pub const CLEAR_DONE: char = '\u{E74D}';
}

/// 待办列表每页几行。
pub const TODO_ROWS: usize = 5;
const TODO_TOP: f32 = 88.0;
const TODO_ROW_HEIGHT: f32 = 31.0;

/// 每页最多显示几条剪贴板记录。
pub const CLIP_ROWS: usize = 5;
const CLIP_TOP: f32 = 80.0;
const CLIP_ROW_HEIGHT: f32 = 32.0;

fn style(family: &HSTRING, size: f32, weight: DWRITE_FONT_WEIGHT, align: Align) -> TextStyle<'_> {
    TextStyle {
        family,
        size,
        weight,
        align,
    }
}

/// 强调色上的文字用黑还是白。
fn on(c: Rgb) -> Rgb {
    color::best_text_on(c)
}

// ───────────────────────────── 音乐胶囊 ─────────────────────────────

/// 竖排文字：一个字一格，从上往下（中文是正的，和设计预览一样）；放不下的用"…"收尾。
/// `column` 是 (中心横坐标, 顶, 底)，`font` 是 (字号, 字重)。
fn vertical_text(
    p: &Painter,
    text: &str,
    (center_x, top, bottom): (f32, f32, f32),
    (size, weight): (f32, DWRITE_FONT_WEIGHT),
    c: windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F,
) -> Result<()> {
    let family = p.gfx.text_font.clone();
    let st = style(&family, size, weight, Align::Center);
    let step = size * 1.18;
    let chars: Vec<char> = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .collect();
    let fit = ((bottom - top) / step).floor().max(0.0) as usize;
    let shown: Vec<char> = if chars.len() > fit {
        chars[..fit.saturating_sub(1)]
            .iter()
            .copied()
            .chain(['…'])
            .collect()
    } else {
        chars
    };
    for (i, ch) in shown.iter().enumerate() {
        if ch.is_whitespace() {
            continue;
        }
        let y = top + i as f32 * step;
        p.text(
            &ch.to_string(),
            Rect::new(center_x - step / 2.0 - 2.0, y, step + 4.0, step),
            &st,
            c,
        )?;
    }
    Ok(())
}

pub fn draw_compact(p: &Painter, v: &ViewState) -> Result<()> {
    let fg = v.palette.foreground;
    let Some(m) = v.media else { return Ok(()) };
    if v.settings.dock.is_side() {
        // 竖胶囊：上面封面，中间竖排歌名，底部是律动条（合成器画）
        let w = SIDE_COMPACT.width;
        draw_art(p, m, Rect::new((w - 26.0) / 2.0, 7.0, 26.0, 26.0), 7.0, v)?;
        if v.ai.iter().any(|m| !m.read) {
            p.fill_circle(w / 2.0 + 13.0, 8.0, 4.5, color(v.palette.background, 1.0))?;
            p.fill_circle(w / 2.0 + 13.0, 8.0, 3.5, color(v.accent, 1.0))?;
        }
        let title = if m.title.is_empty() {
            "正在播放"
        } else {
            &m.title
        };
        return vertical_text(
            p,
            title,
            (w / 2.0, 42.0, SIDE_COMPACT.height - 44.0),
            (12.0, DWRITE_FONT_WEIGHT_SEMI_BOLD),
            color(fg, 0.95),
        );
    }
    draw_art(p, m, Rect::new(7.0, 6.0, 26.0, 26.0), 7.0, v)?;
    // 有没看过的 AI 消息：封面右上角一个小点
    if v.ai.iter().any(|m| !m.read) {
        p.fill_circle(33.0, 7.0, 4.5, color(v.palette.background, 1.0))?;
        p.fill_circle(33.0, 7.0, 3.5, color(v.accent, 1.0))?;
    }
    let family = p.gfx.text_font.clone();
    let title = if m.title.is_empty() {
        "正在播放"
    } else {
        &m.title
    };
    p.text(
        title,
        Rect::new(42.0, 0.0, 170.0, COMPACT.height),
        &style(&family, 12.5, DWRITE_FONT_WEIGHT_SEMI_BOLD, Align::Leading),
        color(fg, 0.95),
    )
}

// ───────────────────────────── 待机时钟 ─────────────────────────────

/// `unread`：有没看过的 AI 消息时，在右边留一个强调色的小点。
pub fn draw_clock(
    p: &Painter,
    palette: &Palette,
    hour: u16,
    minute: u16,
    unread: Option<Rgb>,
    vertical: bool,
) -> Result<()> {
    let family = p.gfx.text_font.clone();
    if vertical {
        // 竖胶囊：时和分上下叠着
        let (w, h) = (SIDE_CLOCK.width, SIDE_CLOCK.height);
        let st = style(&family, 16.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, Align::Center);
        p.text(
            &format!("{hour:02}"),
            Rect::new(0.0, h / 2.0 - 22.0, w, 22.0),
            &st,
            color(palette.foreground, 1.0),
        )?;
        p.text(
            &format!("{minute:02}"),
            Rect::new(0.0, h / 2.0, w, 22.0),
            &st,
            color(palette.foreground, 0.62),
        )?;
        if let Some(c) = unread {
            p.fill_circle(w / 2.0, h - 11.0, 3.5, color(c, 1.0))?;
        }
        return Ok(());
    }
    p.text(
        &clock(hour, minute),
        Rect::new(0.0, 0.0, IDLE_CLOCK.width, IDLE_CLOCK.height),
        &style(&family, 14.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, Align::Center),
        color(palette.foreground, 1.0),
    )?;
    if let Some(c) = unread {
        p.fill_circle(
            IDLE_CLOCK.width - 18.0,
            IDLE_CLOCK.height / 2.0,
            3.5,
            color(c, 1.0),
        )?;
    }
    Ok(())
}

// ───────────────────────────── 音量 / 亮度提示 ─────────────────────────────

pub fn draw_level(
    p: &Painter,
    palette: &Palette,
    accent: Rgb,
    glyph: char,
    value: f64,
    text: &str,
    vertical: bool,
) -> Result<()> {
    let fg = palette.foreground;
    let family = p.gfx.text_font.clone();
    if vertical {
        // 竖胶囊：上面图标，中间竖着的音量条（从下往上涨），底部是数字
        let (w, h) = (SIDE_LEVEL.width, SIDE_LEVEL.height);
        p.icon(glyph, Rect::new(0.0, 8.0, w, 30.0), 15.0, color(fg, 1.0))?;
        let track = Rect::new(w / 2.0 - 3.0, 44.0, 6.0, h - 44.0 - 40.0);
        p.fill_round(track, 3.0, color(fg, 0.18))?;
        let filled =
            (track.h * value.clamp(0.0, 1.0) as f32).max(if value > 0.0 { 6.0 } else { 0.0 });
        if filled > 0.0 {
            p.fill_round(
                Rect::new(track.x, track.bottom() - filled, track.w, filled),
                3.0,
                color(accent, 1.0),
            )?;
        }
        let number = text.trim_end_matches('%');
        return p.text(
            number,
            Rect::new(0.0, h - 34.0, w, 24.0),
            &style(&family, 12.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, Align::Center),
            color(fg, 0.9),
        );
    }
    let h = LEVEL.height;
    p.icon(glyph, Rect::new(10.0, 0.0, 34.0, h), 15.0, color(fg, 1.0))?;
    let track = Rect::new(50.0, h / 2.0 - 2.0, 190.0, 4.0);
    p.fill_round(track, 2.0, color(fg, 0.18))?;
    let filled = (track.w * value.clamp(0.0, 1.0) as f32).max(if value > 0.0 { 4.0 } else { 0.0 });
    if filled > 0.0 {
        p.fill_round(
            Rect::new(track.x, track.y, filled, track.h),
            2.0,
            color(accent, 1.0),
        )?;
    }
    p.text(
        text,
        Rect::new(244.0, 0.0, 46.0, h),
        &style(&family, 12.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, Align::Trailing),
        color(fg, 0.9),
    )
}

// ───────────────────────────── 通知 ─────────────────────────────

/// 通知的图标：软件自己的图标；没有就画一个字形。
pub struct NoteIcon<'a> {
    pub glyph: char,
    pub image: Option<&'a Art>,
}

pub fn draw_notification(
    p: &Painter,
    palette: &Palette,
    accent: Rgb,
    NoteIcon { glyph, image: icon }: NoteIcon,
    title: &str,
    body: &str,
    vertical: bool,
) -> Result<()> {
    let fg = palette.foreground;
    if vertical {
        // 竖胶囊：上面图标，下面两列竖排文字，右边一列是标题、左边一列是正文（中文竖排从右往左读）
        let (w, h) = (SIDE_NOTIFICATION.width, SIDE_NOTIFICATION.height);
        let r = Rect::new(w / 2.0 - 17.0, 14.0, 34.0, 34.0);
        match icon {
            Some(a) => p.image(a.id, &a.pixels, a.width, a.height, r, 8.0)?,
            None => {
                p.fill_circle(w / 2.0, r.y + 17.0, 17.0, color(accent, 1.0))?;
                p.icon(glyph, r, 15.0, color(on(accent), 1.0))?;
            }
        }
        let (top, bottom) = (r.bottom() + 10.0, h - 16.0);
        vertical_text(
            p,
            title,
            (w / 2.0 + 9.0, top, bottom),
            (12.5, DWRITE_FONT_WEIGHT_SEMI_BOLD),
            color(fg, 1.0),
        )?;
        return vertical_text(
            p,
            body,
            (w / 2.0 - 9.0, top, bottom),
            (11.5, DWRITE_FONT_WEIGHT_NORMAL),
            color(fg, 0.62),
        );
    }
    let h = NOTIFICATION.height;
    match icon {
        // 软件自己的图标
        Some(a) => p.image(
            a.id,
            &a.pixels,
            a.width,
            a.height,
            Rect::new(14.0, h / 2.0 - 20.0, 40.0, 40.0),
            9.0,
        )?,
        None => {
            p.fill_circle(34.0, h / 2.0, 20.0, color(accent, 1.0))?;
            p.icon(
                glyph,
                Rect::new(14.0, h / 2.0 - 20.0, 40.0, 40.0),
                17.0,
                color(on(accent), 1.0),
            )?;
        }
    }
    let family = p.gfx.text_font.clone();
    p.text(
        title,
        Rect::new(66.0, 12.0, 256.0, 20.0),
        &style(&family, 13.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, Align::Leading),
        color(fg, 1.0),
    )?;
    p.text(
        body,
        Rect::new(66.0, 32.0, 256.0, 20.0),
        &style(&family, 12.0, DWRITE_FONT_WEIGHT_NORMAL, Align::Leading),
        color(fg, 0.65),
    )
}

// ───────────────────────────── 展开面板 ─────────────────────────────

pub fn draw_expanded(p: &Painter, v: &ViewState) -> Result<Targets> {
    let mut t = Targets::default();
    draw_header(p, v, &mut t)?;
    match v.tab {
        Tab::Music => match v.media {
            Some(m) => draw_player(p, m, v, &mut t)?,
            None => draw_empty_music(p, v)?,
        },
        Tab::Todo => draw_todos(p, v, &mut t)?,
        Tab::Clipboard => draw_clipboard(p, v, &mut t)?,
        Tab::Shelf => draw_shelf(p, v, &mut t)?,
        Tab::Translate if v.translate.settings_open => draw_translate_settings(p, v, &mut t)?,
        Tab::Translate => draw_translate(p, v, &mut t)?,
        Tab::Monitor => draw_monitor(p, v)?,
        Tab::Appearance => draw_appearance(p, v, &mut t)?,
        Tab::Ai => draw_ai(p, v, &mut t)?,
    }
    Ok(t)
}

// ───────────────────────────── AI 动态 ─────────────────────────────

fn draw_ai(p: &Painter, v: &ViewState, t: &mut Targets) -> Result<()> {
    let fg = v.palette.foreground;
    let family = p.gfx.text_font.clone();
    let w = EXPANDED.width;
    if v.ai.is_empty() {
        p.icon(
            GLYPH_AI,
            Rect::new(0.0, 100.0, w, 32.0),
            24.0,
            color(fg, 0.3),
        )?;
        p.text(
            "还没有 AI 消息",
            Rect::new(0.0, 136.0, w, 22.0),
            &style(&family, 13.0, DWRITE_FONT_WEIGHT_NORMAL, Align::Center),
            color(fg, 0.5),
        )?;
        return p.text(
            "Claude、Codex 等完成任务或等你确认时会显示在这里",
            Rect::new(0.0, 160.0, w, 18.0),
            &style(&family, 11.0, DWRITE_FONT_WEIGHT_NORMAL, Align::Center),
            color(fg, 0.4),
        );
    }
    let unread = v.ai.iter().filter(|m| !m.read).count();
    let summary = if unread > 0 {
        format!("{unread} 条没看过 · 点一条标记已读")
    } else {
        "都看过了".to_string()
    };
    p.text(
        &summary,
        Rect::new(16.0, 50.0, 300.0, 24.0),
        &style(&family, 11.0, DWRITE_FONT_WEIGHT_NORMAL, Align::Leading),
        color(fg, 0.5),
    )?;
    let (label, hit) = if unread > 0 {
        ("全部已读", Hit::AiReadAll)
    } else {
        ("清空", Hit::AiClear)
    };
    let bw = segment_width(label);
    let b = Rect::new(w - 16.0 - bw, 51.0, bw, 22.0);
    let hot = v.hovered == Some(hit);
    p.fill_round(b, 11.0, color(fg, if hot { 0.18 } else { 0.1 }))?;
    p.text(
        label,
        b,
        &style(&family, 11.5, DWRITE_FONT_WEIGHT_NORMAL, Align::Center),
        color(fg, 0.85),
    )?;
    t.add(b, hit);

    let title = style(&family, 12.5, DWRITE_FONT_WEIGHT_SEMI_BOLD, Align::Leading);
    let title_read = style(&family, 12.5, DWRITE_FONT_WEIGHT_NORMAL, Align::Leading);
    let body = style(&family, 11.5, DWRITE_FONT_WEIGHT_NORMAL, Align::Leading);
    let time = style(&family, 10.5, DWRITE_FONT_WEIGHT_NORMAL, Align::Trailing);
    for (i, m) in v.ai.iter().take(AI_ROWS).enumerate() {
        let y = AI_TOP + i as f32 * AI_ROW_HEIGHT;
        let r = Rect::new(12.0, y, 416.0, AI_ROW_HEIGHT - 3.0);
        let hovered = v.hovered == Some(Hit::AiItem(i));
        p.fill_round(r, 10.0, color(fg, if hovered { 0.12 } else { 0.06 }))?;
        let icon = Rect::new(22.0, y + 7.0, 25.0, 25.0);
        match &m.icon {
            Some(a) => p.image(a.id, &a.pixels, a.width, a.height, icon, 6.0)?,
            None => {
                p.fill_circle(icon.x + 12.5, icon.y + 12.5, 12.5, color(v.accent, 1.0))?;
                p.icon(GLYPH_AI, icon, 12.0, color(on(v.accent), 1.0))?;
            }
        }
        p.text(
            &m.title,
            Rect::new(56.0, y + 3.0, 300.0, 18.0),
            if m.read { &title_read } else { &title },
            color(fg, if m.read { 0.75 } else { 1.0 }),
        )?;
        p.text(
            &m.body,
            Rect::new(56.0, y + 20.0, 340.0, 16.0),
            &body,
            color(fg, 0.55),
        )?;
        p.text(
            &m.at,
            Rect::new(356.0, y + 3.0, 52.0, 18.0),
            &time,
            color(fg, 0.4),
        )?;
        if !m.read {
            p.fill_circle(r.right() - 9.0, y + 27.0, 3.5, color(v.accent, 1.0))?;
        }
        t.add(r, Hit::AiItem(i));
    }
    Ok(())
}

/// AI 消息的图标（Segoe Fluent Icons：机器人）。
pub const GLYPH_AI: char = '\u{E99A}';

fn hover_bg(p: &Painter, v: &ViewState, hit: Hit, r: Rect, radius: f32) -> Result<()> {
    if v.hovered == Some(hit) {
        let a = if v.pressed == Some(hit) { 0.18 } else { 0.10 };
        p.fill_round(r, radius, color(v.palette.foreground, a))?;
    }
    Ok(())
}

/// 标签按钮的间距：8 个标签放在图钉（x = 304）左边。
const TAB_STEP: f32 = 36.0;

fn draw_header(p: &Painter, v: &ViewState, t: &mut Targets) -> Result<()> {
    let fg = v.palette.foreground;
    let family = p.gfx.text_font.clone();
    for (i, tab) in Tab::ALL.into_iter().enumerate() {
        let r = Rect::new(16.0 + i as f32 * TAB_STEP, 12.0, TAB_STEP - 4.0, 28.0);
        if v.tab == tab {
            p.fill_round(r, 14.0, color(fg, 0.20))?;
        } else {
            hover_bg(p, v, Hit::Tab(tab), r, 14.0)?;
        }
        let a = if v.tab == tab { 1.0 } else { 0.6 };
        p.icon(tab.glyph(), r, 14.0, color(fg, a))?;
        // 有没看过的 AI 消息：标签右上角一个小点
        if tab == Tab::Ai && v.ai.iter().any(|m| !m.read) {
            p.fill_circle(r.right() - 7.0, r.y + 7.0, 3.5, color(v.accent, 1.0))?;
        }
        t.add(r, Hit::Tab(tab));
    }

    let pin = Rect::new(304.0, 13.0, 26.0, 26.0);
    hover_bg(p, v, Hit::Pin, pin, 13.0)?;
    let pin_color = if v.pinned {
        color(v.accent, 1.0)
    } else {
        color(fg, 0.6)
    };
    p.icon(
        if v.pinned { glyph::UNPIN } else { glyph::PIN },
        pin,
        12.0,
        pin_color,
    )?;
    t.add(pin, Hit::Pin);

    p.text(
        weekday_zh(v.weekday),
        Rect::new(334.0, 12.0, 36.0, 28.0),
        &style(&family, 11.0, DWRITE_FONT_WEIGHT_NORMAL, Align::Trailing),
        color(fg, 0.4),
    )?;
    p.text(
        &clock(v.hour, v.minute),
        Rect::new(372.0, 12.0, 52.0, 28.0),
        &style(&family, 14.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, Align::Trailing),
        color(fg, 1.0),
    )
}

// ───────────────────────────── 音乐 ─────────────────────────────

/// 音乐标签里进度条（画出来的细条）和它的可点范围（上下放宽，好按）。
pub const PROGRESS_BAR: Rect = Rect::new(16.0, 186.0, 408.0, 4.0);
pub const PROGRESS_TRACK: Rect = Rect::new(16.0, 177.0, 408.0, 22.0);

/// 音乐标签里音量条的轨道（相对胶囊）。
pub const VOLUME_TRACK: Rect = Rect::new(44.0, 220.0, 330.0, 18.0);

fn draw_player(p: &Painter, m: &Snapshot, v: &ViewState, t: &mut Targets) -> Result<()> {
    let fg = v.palette.foreground;
    let family = p.gfx.text_font.clone();
    draw_art(p, m, Rect::new(16.0, 56.0, 112.0, 112.0), 14.0, v)?;

    let title = if m.title.is_empty() {
        "未知歌曲"
    } else {
        &m.title
    };
    p.text(
        title,
        Rect::new(144.0, 58.0, 280.0, 26.0),
        &style(&family, 16.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, Align::Leading),
        color(fg, 1.0),
    )?;
    p.text(
        &m.artist,
        Rect::new(144.0, 84.0, 280.0, 20.0),
        &style(&family, 13.0, DWRITE_FONT_WEIGHT_NORMAL, Align::Leading),
        color(fg, 0.6),
    )?;
    p.text(
        &m.app,
        Rect::new(144.0, 104.0, 280.0, 16.0),
        &style(&family, 11.0, DWRITE_FONT_WEIGHT_NORMAL, Align::Leading),
        color(fg, 0.4),
    )?;

    for (r, hit, g, size, enabled) in [
        (
            Rect::new(144.0, 128.0, 40.0, 40.0),
            Hit::Previous,
            glyph::PREVIOUS,
            16.0,
            m.can_previous,
        ),
        (
            Rect::new(192.0, 124.0, 48.0, 48.0),
            Hit::PlayPause,
            if m.playing { glyph::PAUSE } else { glyph::PLAY },
            20.0,
            true,
        ),
        (
            Rect::new(248.0, 128.0, 40.0, 40.0),
            Hit::Next,
            glyph::NEXT,
            16.0,
            m.can_next,
        ),
    ] {
        if hit == Hit::PlayPause {
            let a = if v.pressed == Some(hit) {
                0.18
            } else if v.hovered == Some(hit) {
                0.14
            } else {
                0.10
            };
            p.fill_circle(r.center_x(), r.center_y(), r.w / 2.0, color(fg, a))?;
        } else {
            hover_bg(p, v, hit, r, r.w / 2.0)?;
        }
        p.icon(g, r, size, color(fg, if enabled { 1.0 } else { 0.3 }))?;
        if enabled {
            t.add(r, hit);
        }
    }

    // 进度：播放器不报告进度时由 app 自己估算；不知道总时长时只显示已播放时间
    if let Some((position, duration)) = v.progress {
        let bar = PROGRESS_BAR;
        let small = style(&family, 11.0, DWRITE_FONT_WEIGHT_NORMAL, Align::Leading);
        let total = duration.filter(|d| *d > 0.0);
        // 拖动中显示预览的位置
        let position = match (v.seek, total) {
            (Some(f), Some(t)) => f * t,
            _ => position,
        };
        if let Some(total) = total {
            let fraction = (position / total).clamp(0.0, 1.0) as f32;
            let seekable = m.can_seek;
            p.fill_round(bar, 2.0, color(fg, 0.15))?;
            if fraction > 0.0 {
                p.fill_round(
                    Rect::new(bar.x, bar.y, (bar.w * fraction).max(4.0), bar.h),
                    2.0,
                    color(v.accent, 1.0),
                )?;
            }
            if seekable {
                // 指着或拖着时出现圆点，提示可以拖
                let active = matches!(v.hovered, Some(Hit::Seek)) || v.seek.is_some();
                if active {
                    p.fill_circle(
                        bar.x + bar.w * fraction,
                        bar.y + bar.h / 2.0,
                        6.0,
                        color(fg, 1.0),
                    )?;
                }
                t.add(PROGRESS_TRACK, Hit::Seek);
            }
            let right = TextStyle {
                align: Align::Trailing,
                ..small
            };
            p.text(
                &format_duration(total),
                Rect::new(344.0, 192.0, 80.0, 16.0),
                &right,
                color(fg, 0.5),
            )?;
        }
        p.text(
            &format_duration(position),
            Rect::new(16.0, 192.0, 120.0, 16.0),
            &small,
            color(fg, 0.5),
        )?;
    }

    // 系统音量：点击或拖动调节，也可以在岛上滚动滚轮
    if let Some((level_value, muted)) = v.volume {
        let row_y = VOLUME_TRACK.center_y();
        p.icon(
            level::volume_glyph(level_value, muted),
            Rect::new(16.0, row_y - 9.0, 20.0, 18.0),
            12.0,
            color(fg, 0.6),
        )?;
        let track = Rect::new(VOLUME_TRACK.x, row_y - 2.0, VOLUME_TRACK.w, 4.0);
        p.fill_round(track, 2.0, color(fg, 0.15))?;
        let value = if muted { 0.0 } else { level_value };
        if value > 0.0 {
            p.fill_round(
                Rect::new(track.x, track.y, (track.w * value as f32).max(4.0), track.h),
                2.0,
                color(fg, 0.6),
            )?;
        }
        p.text(
            &level::percent(level_value, muted),
            Rect::new(380.0, row_y - 9.0, 44.0, 18.0),
            &style(&family, 11.0, DWRITE_FONT_WEIGHT_NORMAL, Align::Trailing),
            color(fg, 0.5),
        )?;
        t.add(VOLUME_TRACK, Hit::Volume);
    }
    Ok(())
}

fn draw_empty_music(p: &Painter, v: &ViewState) -> Result<()> {
    let fg = v.palette.foreground;
    let family = p.gfx.text_font.clone();
    let w = EXPANDED.width;
    p.icon(
        glyph::MUSIC,
        Rect::new(0.0, 92.0, w, 36.0),
        28.0,
        color(fg, 0.3),
    )?;
    p.text(
        "没有正在播放的音乐",
        Rect::new(0.0, 134.0, w, 22.0),
        &style(&family, 13.0, DWRITE_FONT_WEIGHT_NORMAL, Align::Center),
        color(fg, 0.6),
    )?;
    p.text(
        "在 Spotify、网易云音乐、QQ 音乐或浏览器里播放后会显示在这里",
        Rect::new(16.0, 158.0, w - 32.0, 18.0),
        &style(&family, 11.0, DWRITE_FONT_WEIGHT_NORMAL, Align::Center),
        color(fg, 0.4),
    )
}

fn draw_art(p: &Painter, m: &Snapshot, r: Rect, radius: f32, v: &ViewState) -> Result<()> {
    match &m.art {
        Some(art) => p.image(art.id, &art.pixels, art.width, art.height, r, radius),
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

// ───────────────────────────── 输入框 ─────────────────────────────

/// 画一个输入框并登记点击区域。`value` 是没在编辑时显示的内容。
fn draw_field(
    p: &Painter,
    v: &ViewState,
    t: &mut Targets,
    field: Field,
    rect: Rect,
    value: &str,
    placeholder: &str,
) -> Result<()> {
    let draw = |text: &str,
                caret: Option<usize>,
                selection: Option<(usize, usize)>,
                composition: Option<(usize, usize)>| {
        p.field(&FieldDraw {
            text,
            placeholder,
            rect,
            size: field.font_size(),
            multiline: field.multiline(),
            foreground: v.palette.foreground,
            accent: v.accent,
            caret,
            selection,
            composition,
        })
    };
    let geometry = match v.editing.as_ref().filter(|e| e.field == field) {
        Some(e) => {
            let edit = e.edit;
            let caret = edit.utf16_index(edit.caret());
            if e.composition.is_empty() {
                let selection = edit
                    .selection()
                    .map(|(a, b)| (edit.utf16_index(a), edit.utf16_index(b)));
                draw(
                    &display_text(field, edit.text()),
                    Some(caret),
                    selection,
                    None,
                )?
            } else {
                // 组字内容先插在光标处显示，上屏后才真正写进去
                let mut text = edit.text().to_string();
                text.insert_str(edit.caret(), e.composition);
                let len = e.composition.encode_utf16().count();
                draw(
                    &text,
                    Some(caret + e.composition_caret.min(len)),
                    None,
                    Some((caret, caret + len)),
                )?
            }
        }
        None => draw(&display_text(field, value), None, None, None)?,
    };
    t.add(rect, Hit::Field(field));
    t.fields.push((field, geometry));
    Ok(())
}

// ───────────────────────────── 待办 ─────────────────────────────

fn draw_todos(p: &Painter, v: &ViewState, t: &mut Targets) -> Result<()> {
    let fg = v.palette.foreground;
    let family = p.gfx.text_font.clone();
    draw_field(
        p,
        v,
        t,
        Field::Todo,
        Rect::new(16.0, 50.0, 372.0, 30.0),
        "",
        "添加待办，回车确认",
    )?;
    let clear = Rect::new(396.0, 52.0, 26.0, 26.0);
    if v.todos.iter().any(|todo| todo.done) {
        hover_bg(p, v, Hit::TodoClearDone, clear, 13.0)?;
        p.icon(glyph::CLEAR_DONE, clear, 12.0, color(fg, 0.6))?;
        t.add(clear, Hit::TodoClearDone);
    }

    if v.todos.is_empty() {
        let w = EXPANDED.width;
        p.icon(
            glyph::TODO,
            Rect::new(0.0, 124.0, w, 32.0),
            24.0,
            color(fg, 0.3),
        )?;
        return p.text(
            "今天还没有待办",
            Rect::new(0.0, 160.0, w, 22.0),
            &style(&family, 13.0, DWRITE_FONT_WEIGHT_NORMAL, Align::Center),
            color(fg, 0.5),
        );
    }

    let text = style(&family, 13.0, DWRITE_FONT_WEIGHT_NORMAL, Align::Leading);
    for (row, (i, todo)) in v
        .todos
        .iter()
        .enumerate()
        .skip(v.todo_scroll)
        .take(TODO_ROWS)
        .enumerate()
    {
        let y = TODO_TOP + row as f32 * TODO_ROW_HEIGHT;
        let r = Rect::new(12.0, y, 416.0, TODO_ROW_HEIGHT - 4.0);
        let hovered = matches!(v.hovered, Some(Hit::TodoToggle(h) | Hit::TodoDelete(h)) if h == i);
        p.fill_round(r, 10.0, color(fg, if hovered { 0.12 } else { 0.06 }))?;

        // 勾选框
        let (cx, cy) = (r.x + 18.0, r.center_y());
        if todo.done {
            p.fill_circle(cx, cy, 8.0, color(v.accent, 1.0))?;
            p.icon(
                glyph::CHECK,
                Rect::new(cx - 8.0, cy - 8.0, 16.0, 16.0),
                9.0,
                color(on(v.accent), 1.0),
            )?;
        } else {
            p.fill_circle(cx, cy, 8.0, color(fg, 0.45))?;
            p.fill_circle(cx, cy, 6.5, color(v.palette.background, 1.0))?;
            p.fill_circle(cx, cy, 6.5, color(fg, 0.06))?;
        }
        t.add(r, Hit::TodoToggle(i));

        let text_rect = Rect::new(r.x + 34.0, y, r.w - 70.0, r.h);
        if todo.done {
            p.text_struck(&todo.text, text_rect, &text, color(fg, 0.4))?;
        } else {
            p.text(&todo.text, text_rect, &text, color(fg, 0.95))?;
        }
        if hovered {
            let del = Rect::new(r.right() - 30.0, y + 1.5, 24.0, 24.0);
            hover_bg(p, v, Hit::TodoDelete(i), del, 12.0)?;
            p.icon(glyph::CLOSE, del, 10.0, color(fg, 0.7))?;
            t.add(del, Hit::TodoDelete(i));
        }
    }

    let pending = v.todos.iter().filter(|todo| !todo.done).count();
    let footer = if v.todos.len() > TODO_ROWS {
        format!(
            "还有 {pending} 项未完成 · {}–{} / {}，滚动滚轮查看更多",
            v.todo_scroll + 1,
            (v.todo_scroll + TODO_ROWS).min(v.todos.len()),
            v.todos.len()
        )
    } else if pending > 0 {
        format!("还有 {pending} 项未完成")
    } else {
        "全部完成".to_string()
    };
    p.text(
        &footer,
        Rect::new(16.0, 244.0, 408.0, 12.0),
        &style(&family, 10.0, DWRITE_FONT_WEIGHT_NORMAL, Align::Center),
        color(fg, 0.35),
    )
}

// ───────────────────────────── 剪贴板 ─────────────────────────────

fn draw_clipboard(p: &Painter, v: &ViewState, t: &mut Targets) -> Result<()> {
    let fg = v.palette.foreground;
    let family = p.gfx.text_font.clone();
    p.text(
        "最近复制的文字，点一下重新复制",
        Rect::new(16.0, 50.0, 340.0, 24.0),
        &style(&family, 11.0, DWRITE_FONT_WEIGHT_NORMAL, Align::Leading),
        color(fg, 0.4),
    )?;

    if v.clips.is_empty() {
        let w = EXPANDED.width;
        p.icon(
            glyph::CLIPBOARD,
            Rect::new(0.0, 112.0, w, 32.0),
            24.0,
            color(fg, 0.3),
        )?;
        return p.text(
            "还没有复制过文字",
            Rect::new(0.0, 148.0, w, 22.0),
            &style(&family, 13.0, DWRITE_FONT_WEIGHT_NORMAL, Align::Center),
            color(fg, 0.5),
        );
    }

    let clear = Rect::new(398.0, 49.0, 26.0, 26.0);
    hover_bg(p, v, Hit::ClipClear, clear, 13.0)?;
    p.icon(glyph::DELETE, clear, 12.0, color(fg, 0.6))?;
    t.add(clear, Hit::ClipClear);

    let text = style(&family, 13.0, DWRITE_FONT_WEIGHT_NORMAL, Align::Leading);
    let time = style(&family, 11.0, DWRITE_FONT_WEIGHT_NORMAL, Align::Trailing);
    for (row, (i, entry)) in v
        .clips
        .iter()
        .enumerate()
        .skip(v.clip_scroll)
        .take(CLIP_ROWS)
        .enumerate()
    {
        let y = CLIP_TOP + row as f32 * CLIP_ROW_HEIGHT;
        let r = Rect::new(12.0, y, 416.0, CLIP_ROW_HEIGHT - 4.0);
        let hovered = matches!(v.hovered, Some(Hit::ClipItem(h) | Hit::ClipDelete(h)) if h == i);
        p.fill_round(r, 10.0, color(fg, if hovered { 0.12 } else { 0.06 }))?;
        p.text(
            &entry.preview(),
            Rect::new(24.0, y, 320.0, r.h),
            &text,
            color(fg, 0.95),
        )?;
        t.add(r, Hit::ClipItem(i));
        if hovered {
            let del = Rect::new(r.right() - 30.0, y + 2.0, 24.0, 24.0);
            hover_bg(p, v, Hit::ClipDelete(i), del, 12.0)?;
            p.icon(glyph::CLOSE, del, 10.0, color(fg, 0.7))?;
            t.add(del, Hit::ClipDelete(i));
        } else {
            p.text(
                &entry.time_label(v.today),
                Rect::new(344.0, y, 72.0, r.h),
                &time,
                color(fg, 0.4),
            )?;
        }
    }

    if v.clips.len() > CLIP_ROWS {
        let shown_to = (v.clip_scroll + CLIP_ROWS).min(v.clips.len());
        p.text(
            &format!(
                "{}–{} / {}，滚动滚轮查看更多",
                v.clip_scroll + 1,
                shown_to,
                v.clips.len()
            ),
            Rect::new(16.0, 240.0, 408.0, 14.0),
            &style(&family, 10.0, DWRITE_FONT_WEIGHT_NORMAL, Align::Center),
            color(fg, 0.35),
        )?;
    }
    Ok(())
}

// ───────────────────────────── 中转站 ─────────────────────────────

/// 中转站每页几行。
pub const SHELF_ROWS: usize = 5;
const SHELF_TOP: f32 = 80.0;
const SHELF_ROW_HEIGHT: f32 = 32.0;

fn draw_shelf(p: &Painter, v: &ViewState, t: &mut Targets) -> Result<()> {
    let fg = v.palette.foreground;
    let family = p.gfx.text_font.clone();
    let w = EXPANDED.width;
    p.text(
        "拖进来暂存，拖出去使用，双击打开",
        Rect::new(16.0, 50.0, 340.0, 24.0),
        &style(&family, 11.0, DWRITE_FONT_WEIGHT_NORMAL, Align::Leading),
        color(fg, 0.4),
    )?;

    if v.drag_over {
        let r = Rect::new(12.0, 78.0, 416.0, 166.0);
        p.fill_round(r, 14.0, color(v.accent, 0.18))?;
        p.icon(
            '\u{E896}',
            Rect::new(0.0, 120.0, w, 36.0),
            26.0,
            color(v.accent, 1.0),
        )?;
        return p.text(
            "松开就放进中转站",
            Rect::new(0.0, 160.0, w, 22.0),
            &style(&family, 13.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, Align::Center),
            color(fg, 0.9),
        );
    }

    if v.shelf.is_empty() {
        p.icon(
            '\u{E898}',
            Rect::new(0.0, 112.0, w, 32.0),
            24.0,
            color(fg, 0.3),
        )?;
        return p.text(
            "把文件拖到灵动岛上",
            Rect::new(0.0, 148.0, w, 22.0),
            &style(&family, 13.0, DWRITE_FONT_WEIGHT_NORMAL, Align::Center),
            color(fg, 0.5),
        );
    }

    let clear = Rect::new(398.0, 49.0, 26.0, 26.0);
    hover_bg(p, v, Hit::ShelfClear, clear, 13.0)?;
    p.icon(glyph::DELETE, clear, 12.0, color(fg, 0.6))?;
    t.add(clear, Hit::ShelfClear);

    let name = style(&family, 13.0, DWRITE_FONT_WEIGHT_NORMAL, Align::Leading);
    let detail = style(&family, 11.0, DWRITE_FONT_WEIGHT_NORMAL, Align::Trailing);
    for (row, (i, item)) in v
        .shelf
        .iter()
        .enumerate()
        .skip(v.shelf_scroll)
        .take(SHELF_ROWS)
        .enumerate()
    {
        let y = SHELF_TOP + row as f32 * SHELF_ROW_HEIGHT;
        let r = Rect::new(12.0, y, 416.0, SHELF_ROW_HEIGHT - 4.0);
        let hovered = matches!(
            v.hovered,
            Some(Hit::ShelfItem(h) | Hit::ShelfReveal(h) | Hit::ShelfRemove(h)) if h == i
        );
        p.fill_round(r, 10.0, color(fg, if hovered { 0.12 } else { 0.06 }))?;
        p.icon(
            item.glyph(),
            Rect::new(r.x + 6.0, y, 26.0, r.h),
            13.0,
            color(v.accent, 1.0),
        )?;
        p.text(
            item.name(),
            Rect::new(r.x + 36.0, y, 250.0, r.h),
            &name,
            color(fg, 0.95),
        )?;
        t.add(r, Hit::ShelfItem(i));
        if hovered {
            let reveal = Rect::new(r.right() - 58.0, y + 2.0, 24.0, 24.0);
            hover_bg(p, v, Hit::ShelfReveal(i), reveal, 12.0)?;
            p.icon('\u{E8DA}', reveal, 11.0, color(fg, 0.7))?;
            t.add(reveal, Hit::ShelfReveal(i));
            let remove = Rect::new(r.right() - 30.0, y + 2.0, 24.0, 24.0);
            hover_bg(p, v, Hit::ShelfRemove(i), remove, 12.0)?;
            p.icon(glyph::CLOSE, remove, 10.0, color(fg, 0.7))?;
            t.add(remove, Hit::ShelfRemove(i));
        } else {
            p.text(
                item.detail(),
                Rect::new(r.right() - 120.0, y, 110.0, r.h),
                &detail,
                color(fg, 0.4),
            )?;
        }
    }

    if v.shelf.len() > SHELF_ROWS {
        p.text(
            &format!(
                "{}–{} / {}，滚动滚轮查看更多",
                v.shelf_scroll + 1,
                (v.shelf_scroll + SHELF_ROWS).min(v.shelf.len()),
                v.shelf.len()
            ),
            Rect::new(16.0, 240.0, 408.0, 14.0),
            &style(&family, 10.0, DWRITE_FONT_WEIGHT_NORMAL, Align::Center),
            color(fg, 0.35),
        )?;
    }
    Ok(())
}

// ───────────────────────────── 翻译 ─────────────────────────────

/// 圆形图标按钮。
fn icon_button(
    p: &Painter,
    v: &ViewState,
    t: &mut Targets,
    r: Rect,
    g: char,
    hit: Hit,
) -> Result<()> {
    hover_bg(p, v, hit, r, r.h / 2.0)?;
    p.icon(g, r, 12.0, color(v.palette.foreground, 0.7))?;
    t.add(r, hit);
    Ok(())
}

/// 文字按钮；`primary` 时用强调色。
#[allow(clippy::too_many_arguments)]
fn text_button(
    p: &Painter,
    v: &ViewState,
    t: &mut Targets,
    r: Rect,
    text: &str,
    primary: bool,
    hit: Hit,
) -> Result<()> {
    let fg = v.palette.foreground;
    let hovered = v.hovered == Some(hit);
    let (bg, text_color) = if primary {
        (
            color(v.accent, if hovered { 0.85 } else { 1.0 }),
            color(on(v.accent), 1.0),
        )
    } else {
        (
            color(fg, if hovered { 0.16 } else { 0.08 }),
            color(fg, 0.95),
        )
    };
    p.fill_round(r, r.h / 2.0, bg)?;
    let family = p.gfx.text_font.clone();
    p.text(
        text,
        r,
        &style(&family, 12.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, Align::Center),
        text_color,
    )?;
    t.add(r, hit);
    Ok(())
}

const ERROR_RED: Rgb = Rgb::new(0xFF, 0x45, 0x3A);

/// 译文区域（相对胶囊）。
pub const OUTPUT_RECT: Rect = Rect::new(16.0, 170.0, 408.0, 76.0);

fn draw_translate(p: &Painter, v: &ViewState, t: &mut Targets) -> Result<()> {
    let fg = v.palette.foreground;
    let family = p.gfx.text_font.clone();
    let tu = v.translate;
    draw_field(
        p,
        v,
        t,
        Field::TranslateInput,
        Rect::new(16.0, 50.0, 408.0, 78.0),
        &tu.input,
        "输入要翻译的文字，Enter 翻译，Shift+Enter 换行",
    )?;

    // 状态 + 按钮
    let y = 136.0;
    let (status, status_color) = match &tu.status {
        Status::Busy => ("正在翻译…".to_string(), color(fg, 0.6)),
        Status::Error(e) => (e.clone(), color(ERROR_RED, 1.0)),
        Status::Idle if !v.translate_summary.1 => (
            "还没有设置翻译服务，点右边的齿轮".to_string(),
            color(fg, 0.5),
        ),
        Status::Idle => (v.translate_summary.0.clone(), color(fg, 0.4)),
    };
    p.text(
        &status,
        Rect::new(16.0, y, 236.0, 26.0),
        &style(&family, 11.0, DWRITE_FONT_WEIGHT_NORMAL, Align::Leading),
        status_color,
    )?;
    icon_button(
        p,
        v,
        t,
        Rect::new(258.0, y, 26.0, 26.0),
        '\u{E713}',
        Hit::TranslateSettings,
    )?;
    icon_button(
        p,
        v,
        t,
        Rect::new(288.0, y, 26.0, 26.0),
        glyph::CLIPBOARD,
        Hit::TranslatePaste,
    )?;
    if !tu.output.is_empty() {
        icon_button(
            p,
            v,
            t,
            Rect::new(318.0, y, 26.0, 26.0),
            '\u{E8C8}',
            Hit::TranslateCopy,
        )?;
    }
    let busy = tu.status == Status::Busy;
    text_button(
        p,
        v,
        t,
        Rect::new(352.0, y, 72.0, 26.0),
        if busy { "停止" } else { "翻译" },
        !busy,
        Hit::TranslateGo,
    )?;

    if tu.output.is_empty() {
        return Ok(());
    }
    let (scroll, max) = p.paragraph(
        &tu.output,
        OUTPUT_RECT,
        13.0,
        color(fg, 0.95),
        tu.output_scroll,
    )?;
    t.output = Some((OUTPUT_RECT, scroll, max));
    Ok(())
}

fn draw_translate_settings(p: &Painter, v: &ViewState, t: &mut Targets) -> Result<()> {
    let fg = v.palette.foreground;
    let family = p.gfx.text_font.clone();
    let d = &v.translate.draft;

    // 服务：放不下就换行
    let (mut x, mut y) = (16.0, 48.0);
    for (i, preset) in PRESETS.iter().enumerate() {
        let w = segment_width(preset.name);
        if x + w > 424.0 {
            x = 16.0;
            y += 26.0;
        }
        x = segment(
            p,
            v,
            t,
            x,
            y,
            preset.name,
            preset.id.eq_ignore_ascii_case(&d.provider),
            Hit::Provider(i as u8),
        )?;
    }

    let row = |y: f32, name: &str| -> Result<()> {
        p.text(
            name,
            Rect::new(16.0, y, 40.0, 22.0),
            &style(&family, 12.0, DWRITE_FONT_WEIGHT_NORMAL, Align::Leading),
            color(fg, 0.6),
        )
    };
    let y = 104.0;
    row(y, "格式")?;
    let x1 = segment(
        p,
        v,
        t,
        56.0,
        y,
        "Anthropic",
        d.format == Format::Anthropic,
        Hit::ApiFormat(Format::Anthropic),
    )?;
    segment(
        p,
        v,
        t,
        x1,
        y,
        "OpenAI 兼容",
        d.format == Format::OpenAI,
        Hit::ApiFormat(Format::OpenAI),
    )?;

    let preset = tr::preset(&d.provider);
    row(130.0, "地址")?;
    draw_field(
        p,
        v,
        t,
        Field::BaseUrl,
        Rect::new(56.0, 130.0, 368.0, 22.0),
        &d.base_url,
        "留空使用默认地址",
    )?;
    row(156.0, "模型")?;
    draw_field(
        p,
        v,
        t,
        Field::Model,
        Rect::new(56.0, 156.0, 368.0, 22.0),
        &d.model,
        "模型名称",
    )?;
    row(182.0, "Key")?;
    let key_hint = if d.has_stored_key {
        "已保存（不改就留空）"
    } else {
        preset.key_hint
    };
    draw_field(
        p,
        v,
        t,
        Field::ApiKey,
        Rect::new(56.0, 182.0, 368.0, 22.0),
        d.key.as_deref().unwrap_or(""),
        key_hint,
    )?;

    // 提示：正在改地址时实时显示最终请求的地址
    let base_url = match v.editing.as_ref() {
        Some(e) if e.field == Field::BaseUrl => e.edit.text(),
        _ => d.base_url.as_str(),
    };
    let (hint, error) = match &v.translate.hint {
        Some((h, e)) => (h.clone(), *e),
        None if !tr::is_valid(base_url) => (tr::INVALID_URL_HINT.to_string(), true),
        None => (
            format!("请求 {}", tr::request_url(d.format, base_url)),
            false,
        ),
    };
    p.text(
        &hint,
        Rect::new(16.0, 208.0, 408.0, 16.0),
        &style(&family, 10.5, DWRITE_FONT_WEIGHT_NORMAL, Align::Leading),
        if error {
            color(ERROR_RED, 1.0)
        } else {
            color(fg, 0.4)
        },
    )?;

    icon_button(
        p,
        v,
        t,
        Rect::new(16.0, 228.0, 26.0, 24.0),
        '\u{E72B}',
        Hit::SettingsBack,
    )?;
    let testing = v.translate.testing && v.translate.status == Status::Busy;
    text_button(
        p,
        v,
        t,
        Rect::new(276.0, 228.0, 70.0, 24.0),
        if testing { "测试中…" } else { "测试" },
        false,
        Hit::SettingsTest,
    )?;
    text_button(
        p,
        v,
        t,
        Rect::new(354.0, 228.0, 70.0, 24.0),
        "保存",
        true,
        Hit::SettingsSave,
    )
}

// ───────────────────────────── 系统监控 ─────────────────────────────

fn draw_monitor(p: &Painter, v: &ViewState) -> Result<()> {
    let fg = v.palette.foreground;
    let family = p.gfx.text_font.clone();
    let label = style(&family, 12.0, DWRITE_FONT_WEIGHT_NORMAL, Align::Leading);
    let value = style(&family, 14.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, Align::Trailing);
    let Some(s) = v.sample else {
        return p.text(
            "正在采样…",
            Rect::new(16.0, 120.0, 408.0, 24.0),
            &label,
            color(fg, 0.5),
        );
    };

    let meter = |y: f32, name: &str, text: &str, fraction: f64, detail: &str| -> Result<()> {
        p.text(
            name,
            Rect::new(16.0, y, 120.0, 22.0),
            &label,
            color(fg, 0.6),
        )?;
        p.text(
            text,
            Rect::new(260.0, y, 164.0, 22.0),
            &value,
            color(fg, 1.0),
        )?;
        let bar = Rect::new(16.0, y + 28.0, 408.0, 6.0);
        p.fill_round(bar, 3.0, color(fg, 0.12))?;
        let f = fraction.clamp(0.0, 1.0) as f32;
        if f > 0.0 {
            p.fill_round(
                Rect::new(bar.x, bar.y, (bar.w * f).max(6.0), bar.h),
                3.0,
                color(v.accent, 1.0),
            )?;
        }
        if !detail.is_empty() {
            p.text(
                detail,
                Rect::new(140.0, y, 120.0, 22.0),
                &label,
                color(fg, 0.4),
            )?;
        }
        Ok(())
    };
    meter(58.0, "CPU", &format!("{:.0}%", s.cpu), s.cpu / 100.0, "")?;
    meter(
        112.0,
        "内存",
        &format!("{:.0}%", s.memory),
        s.memory / 100.0,
        &format!("{:.1} / {:.1} GB", s.memory_used_gb, s.memory_total_gb),
    )?;

    p.text(
        "网络",
        Rect::new(16.0, 166.0, 120.0, 22.0),
        &label,
        color(fg, 0.6),
    )?;
    let big = style(&family, 15.0, DWRITE_FONT_WEIGHT_SEMI_BOLD, Align::Leading);
    p.text(
        &format!("↓ {}", speed(s.download)),
        Rect::new(16.0, 194.0, 200.0, 28.0),
        &big,
        color(fg, 1.0),
    )?;
    p.text(
        &format!("↑ {}", speed(s.upload)),
        Rect::new(224.0, 194.0, 200.0, 28.0),
        &big,
        color(fg, 1.0),
    )
}

// ───────────────────────────── 外观 ─────────────────────────────

/// 分段按钮的宽度（按字数估算）。
fn segment_width(text: &str) -> f32 {
    22.0 + text
        .chars()
        .map(|c| if c.is_ascii() { 7.0 } else { 12.0 })
        .sum::<f32>()
}

/// 一个分段按钮：选中时用强调色填充。返回下一个按钮的起点。
#[allow(clippy::too_many_arguments)]
fn segment(
    p: &Painter,
    v: &ViewState,
    t: &mut Targets,
    x: f32,
    y: f32,
    text: &str,
    selected: bool,
    hit: Hit,
) -> Result<f32> {
    let fg = v.palette.foreground;
    let family = p.gfx.text_font.clone();
    let w = segment_width(text);
    let r = Rect::new(x, y, w, 22.0);
    let (bg, text_color) = if selected {
        (color(v.accent, 1.0), color(on(v.accent), 1.0))
    } else if v.hovered == Some(hit) {
        (color(fg, 0.16), color(fg, 1.0))
    } else {
        (color(fg, 0.08), color(fg, 0.9))
    };
    p.fill_round(r, 11.0, bg)?;
    p.text(
        text,
        r,
        &style(&family, 12.0, DWRITE_FONT_WEIGHT_NORMAL, Align::Center),
        text_color,
    )?;
    t.add(r, hit);
    Ok(x + w + 6.0)
}

fn row_label(p: &Painter, v: &ViewState, y: f32, text: &str) -> Result<f32> {
    let family = p.gfx.text_font.clone();
    p.text(
        text,
        Rect::new(16.0, y, 56.0, 22.0),
        &style(&family, 12.0, DWRITE_FONT_WEIGHT_NORMAL, Align::Leading),
        color(v.palette.foreground, 0.6),
    )?;
    Ok(72.0)
}

fn draw_appearance(p: &Painter, v: &ViewState, t: &mut Targets) -> Result<()> {
    let s = v.settings;
    let fg = v.palette.foreground;
    let family = p.gfx.text_font.clone();

    let mut x = 16.0;
    for mode in ThemeMode::ALL {
        x = segment(
            p,
            v,
            t,
            x,
            52.0,
            mode.label(),
            s.theme.mode == mode,
            Hit::Theme(mode),
        )?;
    }

    if s.theme.mode == ThemeMode::Custom {
        let current = [
            &s.theme.custom_background,
            &s.theme.custom_foreground,
            &s.theme.custom_accent,
        ];
        for (kind, (name, presets)) in [
            ("背景", BACKGROUND_PRESETS),
            ("文字", FOREGROUND_PRESETS),
            ("强调色", ACCENT_PRESETS),
        ]
        .into_iter()
        .enumerate()
        {
            let y = 82.0 + kind as f32 * 26.0;
            let x0 = row_label(p, v, y, name)?;
            for (i, hex) in presets.iter().enumerate() {
                let c = color::parse_hex(hex).unwrap_or(v.palette.background);
                let cx = x0 + 11.0 + i as f32 * 28.0;
                let cy = y + 11.0;
                let selected = current[kind].eq_ignore_ascii_case(hex);
                p.fill_circle(cx, cy, 10.0, color(fg, if selected { 1.0 } else { 0.25 }))?;
                p.fill_circle(cx, cy, if selected { 7.5 } else { 9.0 }, color(c, 1.0))?;
                t.add(
                    Rect::new(cx - 11.0, cy - 11.0, 22.0, 22.0),
                    Hit::Swatch(kind as u8, i as u8),
                );
            }
            // 也可以直接填 #RRGGBB
            draw_field(
                p,
                v,
                t,
                Field::Hex(kind as u8),
                Rect::new(252.0, y, 84.0, 22.0),
                current[kind],
                "#RRGGBB",
            )?;
        }
    } else if s.theme.mode == ThemeMode::Schedule {
        let small = style(&family, 11.0, DWRITE_FONT_WEIGHT_NORMAL, Align::Leading);
        let y = 82.0;
        p.text(
            "白天从",
            Rect::new(16.0, y, 40.0, 22.0),
            &small,
            color(fg, 0.6),
        )?;
        draw_field(
            p,
            v,
            t,
            Field::DayStarts,
            Rect::new(56.0, y, 60.0, 22.0),
            &s.theme.day_starts,
            "07:00",
        )?;
        p.text(
            "到",
            Rect::new(124.0, y, 16.0, 22.0),
            &small,
            color(fg, 0.6),
        )?;
        draw_field(
            p,
            v,
            t,
            Field::NightStarts,
            Rect::new(144.0, y, 60.0, 22.0),
            &s.theme.night_starts,
            "19:00",
        )?;
        p.text(
            "用浅色，其余时间用深色",
            Rect::new(212.0, y, 212.0, 22.0),
            &small,
            color(fg, 0.6),
        )?;
    } else {
        let hint = match s.theme.mode {
            ThemeMode::Dark => "黑色的经典灵动岛".to_string(),
            ThemeMode::Light => "浅色胶囊，适合浅色桌面".to_string(),
            ThemeMode::System => "跟随 Windows 的浅色 / 深色应用模式".to_string(),
            ThemeMode::Schedule | ThemeMode::Custom => String::new(),
        };
        p.text(
            &hint,
            Rect::new(16.0, 84.0, 408.0, 20.0),
            &style(&family, 11.0, DWRITE_FONT_WEIGHT_NORMAL, Align::Leading),
            color(fg, 0.4),
        )?;
    }

    let mut y = 164.0;
    let x0 = row_label(p, v, y, "待机时")?;
    let x1 = segment(
        p,
        v,
        t,
        x0,
        y,
        "收成细线",
        s.idle_as_line,
        Hit::IdleStyle(true),
    )?;
    segment(
        p,
        v,
        t,
        x1,
        y,
        "显示时间",
        !s.idle_as_line,
        Hit::IdleStyle(false),
    )?;

    y += 28.0;
    let x0 = row_label(p, v, y, "放音乐时")?;
    let x1 = segment(
        p,
        v,
        t,
        x0,
        y,
        "显示音乐胶囊",
        !s.media_as_line,
        Hit::MediaStyle(false),
    )?;
    segment(
        p,
        v,
        t,
        x1,
        y,
        "也收成细线",
        s.media_as_line,
        Hit::MediaStyle(true),
    )?;

    y += 28.0;
    let mut x = row_label(p, v, y, "位置")?;
    for (edge, name) in [
        (Edge::Top, "顶部"),
        (Edge::Left, "左侧"),
        (Edge::Right, "右侧"),
    ] {
        x = segment(p, v, t, x, y, name, s.dock == edge, Hit::Dock(edge))?;
    }
    // 多块屏幕：点一下换到下一块
    if let Some(label) = &v.monitor_label {
        segment(
            p,
            v,
            t,
            x + 8.0,
            y,
            &format!("{label} ›"),
            false,
            Hit::NextMonitor,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_names_round_trip() {
        for t in Tab::ALL {
            assert_eq!(Tab::parse(t.name()), t);
        }
        assert_eq!(Tab::parse("Nope"), Tab::Music); // 不认识的标签回到音乐
        assert_eq!(display_text(Field::ApiKey, "sk-1"), "••••");
        assert_eq!(display_text(Field::Model, "m"), "m");
    }

    #[test]
    fn hit_targets_prefer_the_topmost() {
        let mut t = Targets::default();
        t.add(Rect::new(0.0, 0.0, 100.0, 30.0), Hit::ClipItem(0));
        t.add(Rect::new(70.0, 2.0, 24.0, 24.0), Hit::ClipDelete(0));
        assert_eq!(t.hit(80.0, 10.0), Some(Hit::ClipDelete(0)));
        assert_eq!(t.hit(10.0, 10.0), Some(Hit::ClipItem(0)));
        assert_eq!(t.hit(10.0, 100.0), None);
        assert!(t.any(|h| h == Hit::ClipDelete(0)) && !t.any(|h| h == Hit::Pin));
    }

    #[test]
    fn panel_content_fits() {
        assert!(CLIP_TOP + CLIP_ROWS as f32 * CLIP_ROW_HEIGHT <= EXPANDED.height - 14.0);
        const {
            assert!(TODO_TOP + TODO_ROWS as f32 * TODO_ROW_HEIGHT <= 244.0);
        }
        assert!(VOLUME_TRACK.bottom() <= EXPANDED.height - 14.0);
        // 最长的一行分段按钮放得下
        let widest = 72.0 + (22.0 + 6.0 * 12.0) + 6.0 + (22.0 + 5.0 * 12.0);
        assert!(widest <= EXPANDED.width - 16.0);
    }
}
