//! 合成树：胶囊形状、阴影、裁剪和所有动画都在这里，交给系统合成器在独立线程上运行。
//!
//! ```text
//! root（DPI 缩放）
//!  └ capsule（位置、大小跟着属性集 p 走）
//!     ├ layer（LayerVisual，带 DropShadow：阴影跟着形状实时变化）
//!     │   └ background（纯色，用圆角矩形裁剪）
//!     └ content（用同样的圆角矩形裁剪）
//!         ├ line（细线的颜色：放音乐时变成封面主题色）
//!         ├ compact（音乐胶囊：封面、歌名、律动条）
//!         ├ level（音量 / 亮度提示）
//!         ├ notification（通知）
//!         └ expanded（展开面板）
//! ```
//!
//! 形状只由属性集 p 的 Size / Radius / Top 三个值决定，状态切换时对它们启动弹簧动画，
//! 其余视觉元素用表达式动画绑定到 p 上。整个过程界面线程不参与逐帧计算。

use std::cell::Cell;

use windows::Foundation::{IReference, PropertyValue, TimeSpan};
use windows::UI::Color;
use windows::UI::Composition::*;
use windows::core::{HSTRING, Interface, Result};
use windows_numerics::{Vector2, Vector3};

use island::color::{Palette, Rgb};
use island::metrics::{self, Edge, IslandState, Metrics, Rect, WINDOW_HEIGHT, WINDOW_WIDTH};

use super::gfx::Gfx;

/// 弹簧参数：合成器的 Period（秒）和阻尼比。展开时带一点回弹，收起时干脆利落。
///
/// 合成器的 Period 是 1/ω（不是一次完整振荡的时间），所以和 WPF 版的 response（≈ 完成时间）
/// 差 2π 倍：WPF 展开 0.42 秒 → 0.42 / 2π ≈ 0.067。冒烟测试会在屏幕上实际量一次用时。
#[derive(Clone, Copy)]
pub struct Spring {
    pub period: f32,
    pub damping: f32,
}

pub const EXPAND: Spring = Spring {
    period: 0.42 / std::f32::consts::TAU,
    damping: 0.74,
};
pub const COLLAPSE: Spring = Spring {
    period: 0.30 / std::f32::consts::TAU,
    damping: 0.92,
};

/// 一块固定尺寸的内容：画在合成表面上，按最终尺寸摆放一次，对齐胶囊贴边的那一侧。
/// 胶囊动画时只是裁出它的可见部分，内容本身不需要重新布局或重画。
pub struct Layer {
    pub visual: SpriteVisual,
    pub surface: CompositionDrawingSurface,
    pub width: f32,
    pub height: f32,
}

/// 当前显示哪块内容。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    Line,
    Clock,
    Compact,
    Level,
    Notification,
    Expanded,
}

pub struct Scene {
    pub compositor: Compositor,
    pub root: ContainerVisual,
    props: CompositionPropertySet,
    capsule: ContainerVisual,
    line: SpriteVisual,
    line_brush: CompositionColorBrush,
    pub level: Layer,
    pub notification: Layer,
    pub clock: Layer,
    edge: Edge,
    bg_brush: CompositionColorBrush,
    edge_brush: CompositionColorBrush,
    pub compact: Layer,
    pub expanded: Layer,
    bars: Vec<SpriteVisual>,
    bar_brush: CompositionColorBrush,
    scale: f32,
    /// 上一次设置的细线染色和律动条状态：没变就不重新启动动画（重启会让动画跳一下）。
    line_state: Cell<Option<(Option<Rgb>, bool)>>,
    caret: SpriteVisual,
    caret_brush: CompositionColorBrush,
    equalizer: Cell<Option<bool>>,
}

fn ui_color(c: Rgb, alpha: u8) -> Color {
    Color {
        A: alpha,
        R: c.r,
        G: c.g,
        B: c.b,
    }
}

fn ms(v: f32) -> TimeSpan {
    TimeSpan {
        Duration: (v * 10_000.0) as i64,
    }
}

impl Scene {
    pub fn new(compositor: &Compositor, gfx: &Gfx, scale: f32, palette: &Palette) -> Result<Self> {
        let c = compositor;
        let root = c.CreateContainerVisual()?;
        root.SetSize(Vector2 {
            X: WINDOW_WIDTH,
            Y: WINDOW_HEIGHT,
        })?;
        root.SetScale(Vector3 {
            X: scale,
            Y: scale,
            Z: 1.0,
        })?;

        let start = metrics::LINE;
        let props = c.CreatePropertySet()?;
        props.InsertScalar(&HSTRING::from("W"), start.width)?;
        props.InsertScalar(&HSTRING::from("H"), start.height)?;
        props.InsertScalar(&HSTRING::from("Radius"), start.radius)?;
        props.InsertScalar(&HSTRING::from("Top"), start.top)?;

        let capsule = c.CreateContainerVisual()?;
        bind(c, &capsule, "Size", "Vector2(p.W, p.H)", &props, &[])?;
        root.Children()?.InsertAtTop(&capsule)?;

        // 描边：比胶囊大一圈的淡色圆角矩形垫在背景下面，浅色胶囊放在白色网页上也看得清边界。
        // 以前用 LayerVisual + DropShadow 做阴影：LayerVisual 在缩放过的父节点下会按未缩放的尺寸
        // 渲染（150% 缩放时背景变小、内容溢出），没有显卡时阴影还会变成黑块，所以不用了。
        let edge_brush = c.CreateColorBrushWithColor(edge_color(palette))?;
        let edge_visual = c.CreateSpriteVisual()?;
        edge_visual.SetBrush(&edge_brush)?;
        bind(
            c,
            &edge_visual,
            "Size",
            "Vector2(p.W + 2 * G, p.H + 2 * G)",
            &props,
            &[("G", EDGE)],
        )?;
        edge_visual.SetOffset(Vector3 {
            X: -EDGE,
            Y: -EDGE,
            Z: 0.0,
        })?;
        edge_visual
            .SetClip(&c.CreateGeometricClipWithGeometry(&rounded_geometry(c, &props, EDGE)?)?)?;
        capsule.Children()?.InsertAtTop(&edge_visual)?;

        // 背景：纯色 SpriteVisual，用圆角矩形裁剪，直接挂在缩放过的树里
        let bg_brush = c.CreateColorBrushWithColor(ui_color(palette.background, 255))?;
        let background = c.CreateSpriteVisual()?;
        background.SetBrush(&bg_brush)?;
        bind(c, &background, "Size", "Vector2(p.W, p.H)", &props, &[])?;
        background
            .SetClip(&c.CreateGeometricClipWithGeometry(&rounded_geometry(c, &props, 0.0)?)?)?;
        capsule.Children()?.InsertAtTop(&background)?;

        // 内容：用同样的形状裁剪
        let content = c.CreateContainerVisual()?;
        bind(c, &content, "Size", "Vector2(p.W, p.H)", &props, &[])?;
        content.SetClip(&c.CreateGeometricClipWithGeometry(&rounded_geometry(c, &props, 0.0)?)?)?;
        capsule.Children()?.InsertAtTop(&content)?;

        // 细线的颜色：铺满整个胶囊，只在细线状态下可见
        let line_brush = c.CreateColorBrushWithColor(ui_color(palette.accent, 255))?;
        let line = c.CreateSpriteVisual()?;
        line.SetBrush(&line_brush)?;
        line.SetOpacity(0.0)?;
        bind(c, &line, "Size", "Vector2(p.W, p.H)", &props, &[])?;
        content.Children()?.InsertAtTop(&line)?;

        let compact = make_layer(c, gfx, metrics::COMPACT, scale)?;
        let level = make_layer(c, gfx, metrics::LEVEL, scale)?;
        let notification = make_layer(c, gfx, metrics::NOTIFICATION, scale)?;
        let clock = make_layer(c, gfx, metrics::IDLE_CLOCK, scale)?;
        content.Children()?.InsertAtTop(&clock.visual)?;
        let expanded = make_layer(c, gfx, metrics::EXPANDED, scale)?;
        content.Children()?.InsertAtTop(&compact.visual)?;
        content.Children()?.InsertAtTop(&level.visual)?;
        content.Children()?.InsertAtTop(&notification.visual)?;
        content.Children()?.InsertAtTop(&expanded.visual)?;

        // 律动条：4 根，动画全在合成器里跑
        let bar_brush = c.CreateColorBrushWithColor(ui_color(palette.accent, 255))?;
        let mut bars = Vec::new();
        for _ in 0..4 {
            let bar = c.CreateSpriteVisual()?;
            bar.SetBrush(&bar_brush)?;
            // 大小、位置由 place_bars 按停靠边设置
            compact.visual.Children()?.InsertAtTop(&bar)?;
            bars.push(bar);
        }

        // 输入框的光标：展开面板上的一根细条，闪烁动画在合成器里跑
        let caret_brush = c.CreateColorBrushWithColor(ui_color(palette.foreground, 255))?;
        let caret = c.CreateSpriteVisual()?;
        caret.SetBrush(&caret_brush)?;
        caret.SetOpacity(0.0)?;
        expanded.visual.Children()?.InsertAtTop(&caret)?;

        let mut scene = Self {
            compositor: c.clone(),
            caret,
            caret_brush,
            root,
            props,
            capsule,
            line,
            line_brush,
            level,
            notification,
            clock,
            edge: Edge::Top,
            bg_brush,
            edge_brush,
            compact,
            expanded,
            bars,
            bar_brush,
            scale,
            line_state: Cell::new(None),
            equalizer: Cell::new(None),
        };
        scene.set_edge(gfx, Edge::Top)?;
        Ok(scene)
    }

    /// 换停靠边：胶囊和各块内容的对齐方式都跟着变；左右两侧的收起状态是竖胶囊，
    /// 内容表面按新尺寸重建（调用方随后重画）。调用后再 `jump_to` 到新形状。
    pub fn set_edge(&mut self, gfx: &Gfx, edge: Edge) -> Result<()> {
        self.edge = edge;
        let scale = self.scale;
        for (layer, state) in [
            (&mut self.compact, IslandState::Compact),
            (&mut self.level, IslandState::Level),
            (&mut self.notification, IslandState::Notification),
        ] {
            resize_layer(&self.compositor, gfx, layer, state.metrics(edge), scale)?;
        }
        let clock = IslandState::Idle.metrics_with(edge, false);
        resize_layer(&self.compositor, gfx, &mut self.clock, clock, scale)?;
        self.place_bars()?;
        let c = &self.compositor;
        let offset = match edge {
            Edge::Top => "Vector3((WW - p.W) / 2, p.Top, 0)",
            Edge::Left => "Vector3(p.Top, (WH - p.H) / 2, 0)",
            Edge::Right => "Vector3(WW - p.W - p.Top, (WH - p.H) / 2, 0)",
        };
        bind(
            c,
            &self.capsule,
            "Offset",
            offset,
            &self.props,
            &[("WW", WINDOW_WIDTH), ("WH", WINDOW_HEIGHT)],
        )?;

        let layer_offset = match edge {
            Edge::Top => "Vector3((p.W - VW) / 2, 0, 0)",
            Edge::Left => "Vector3(0, (p.H - VH) / 2, 0)",
            Edge::Right => "Vector3(p.W - VW, (p.H - VH) / 2, 0)",
        };
        for layer in [
            &self.compact,
            &self.level,
            &self.notification,
            &self.clock,
            &self.expanded,
        ] {
            bind(
                c,
                &layer.visual,
                "Offset",
                layer_offset,
                &self.props,
                &[("VW", layer.width), ("VH", layer.height)],
            )?;
            // 缩放从贴边的一侧展开
            let (cx, cy) = match edge {
                Edge::Top => (layer.width / 2.0, 0.0),
                Edge::Left => (0.0, layer.height / 2.0),
                Edge::Right => (layer.width, layer.height / 2.0),
            };
            layer.visual.SetCenterPoint(Vector3 {
                X: cx,
                Y: cy,
                Z: 0.0,
            })?;
        }
        Ok(())
    }

    /// 细线的颜色：`Some` 时细线染成这个颜色（放音乐时），`None` 时是普通的背景色。
    pub fn set_line_tint(&self, tint: Option<Rgb>, line_visible: bool) -> Result<()> {
        if self.line_state.replace(Some((tint, line_visible))) == Some((tint, line_visible)) {
            return Ok(());
        }
        if let Some(c) = tint {
            self.line_brush.SetColor(ui_color(c, 255))?;
        }
        let a = self.compositor.CreateScalarKeyFrameAnimation()?;
        a.InsertKeyFrame(
            1.0,
            if tint.is_some() && line_visible {
                1.0
            } else {
                0.0
            },
        )?;
        a.SetDuration(ms(if line_visible { 220.0 } else { 80.0 }))?;
        self.line.StartAnimation(&HSTRING::from("Opacity"), &a)
    }

    /// 输入框光标（相对展开面板的 DIP 矩形）；None 隐藏。每次移动都从"亮"开始闪。
    pub fn set_caret(&self, caret: Option<(Rect, Rgb)>) -> Result<()> {
        let opacity = HSTRING::from("Opacity");
        let Some((r, c)) = caret else {
            self.caret.StopAnimation(&opacity)?;
            return self.caret.SetOpacity(0.0);
        };
        self.caret_brush.SetColor(ui_color(c, 255))?;
        self.caret.SetOffset(Vector3 {
            X: r.x,
            Y: r.y,
            Z: 0.0,
        })?;
        self.caret.SetSize(Vector2 { X: r.w, Y: r.h })?;
        let a = self.compositor.CreateScalarKeyFrameAnimation()?;
        a.InsertKeyFrame(0.0, 1.0)?;
        a.InsertKeyFrame(0.5, 1.0)?;
        a.InsertKeyFrame(0.55, 0.0)?;
        a.InsertKeyFrame(0.95, 0.0)?;
        a.InsertKeyFrame(1.0, 1.0)?;
        a.SetDuration(ms(1060.0))?;
        a.SetIterationBehavior(AnimationIterationBehavior::Forever)?;
        self.caret.SetOpacity(1.0)?;
        self.caret.StartAnimation(&opacity, &a)
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// 弹簧动画到新形状。动画进行中再调用会从当前值接着变。
    pub fn animate_to(&self, m: Metrics, spring: Spring) -> Result<()> {
        for (name, value, damping) in [
            ("W", m.width, spring.damping),
            ("H", m.height, spring.damping),
            ("Radius", m.radius, spring.damping.max(0.85)), // 圆角和位置不要回弹
            ("Top", m.top, spring.damping.max(0.85)),
        ] {
            let a = self.compositor.CreateSpringScalarAnimation()?;
            a.SetFinalValue(&boxed(value)?)?;
            a.SetPeriod(ms(spring.period * 1000.0))?;
            a.SetDampingRatio(damping)?;
            self.props.StartAnimation(&HSTRING::from(name), &a)?;
        }
        Ok(())
    }

    /// 直接跳到某个形状（启动时、全屏恢复时）。
    pub fn jump_to(&self, m: Metrics) -> Result<()> {
        for (name, value) in [
            ("W", m.width),
            ("H", m.height),
            ("Radius", m.radius),
            ("Top", m.top),
        ] {
            self.props.StopAnimation(&HSTRING::from(name))?;
            self.props.InsertScalar(&HSTRING::from(name), value)?;
        }
        Ok(())
    }

    /// 内容切换：旧的淡出，新的稍等一下再淡入，并从略小一点放大到原尺寸。
    pub fn show(&self, view: View, growing: bool) -> Result<()> {
        for (layer, v) in [
            (&self.clock, View::Clock),
            (&self.compact, View::Compact),
            (&self.level, View::Level),
            (&self.notification, View::Notification),
            (&self.expanded, View::Expanded),
        ] {
            fade(&self.compositor, &layer.visual, view == v, growing)?;
        }
        Ok(())
    }

    /// 全屏时淡出整个岛。
    pub fn set_hidden(&self, hidden: bool) -> Result<()> {
        let a = self.compositor.CreateScalarKeyFrameAnimation()?;
        a.InsertKeyFrame(1.0, if hidden { 0.0 } else { 1.0 })?;
        a.SetDuration(ms(200.0))?;
        self.root.StartAnimation(&HSTRING::from("Opacity"), &a)
    }

    /// 换主题：背景色在合成器里平滑过渡。
    pub fn set_palette(&self, palette: &Palette) -> Result<()> {
        let a = self.compositor.CreateColorKeyFrameAnimation()?;
        a.InsertKeyFrame(1.0, ui_color(palette.background, 255))?;
        a.SetDuration(ms(320.0))?;
        self.bg_brush.StartAnimation(&HSTRING::from("Color"), &a)?;
        let e = self.compositor.CreateColorKeyFrameAnimation()?;
        e.InsertKeyFrame(1.0, edge_color(palette))?;
        e.SetDuration(ms(320.0))?;
        self.edge_brush.StartAnimation(&HSTRING::from("Color"), &e)
    }

    pub fn set_accent(&self, accent: Rgb) -> Result<()> {
        self.bar_brush.SetColor(ui_color(accent, 255))
    }

    /// 律动条：播放时随机高度来回跳，暂停时缩成短条。
    /// 律动条：顶部是竖着的 4 根，挂在音乐胶囊右侧；左右两侧是横着的 4 根，在竖胶囊底部。
    fn place_bars(&self) -> Result<()> {
        let side = self.edge.is_side();
        let h = self.compact.height;
        for (i, bar) in self.bars.iter().enumerate() {
            let i = i as f32;
            bar.StopAnimation(&HSTRING::from("Scale"))?;
            if side {
                bar.SetSize(Vector2 { X: 14.0, Y: 3.0 })?;
                bar.SetOffset(Vector3 {
                    X: 13.0,
                    Y: h - 36.0 + i * 6.0,
                    Z: 0.0,
                })?;
                bar.SetCenterPoint(Vector3 {
                    X: 0.0,
                    Y: 1.5,
                    Z: 0.0,
                })?;
            } else {
                bar.SetSize(Vector2 { X: 3.0, Y: 14.0 })?;
                bar.SetOffset(Vector3 {
                    X: 222.0 + i * 6.0,
                    Y: 12.0,
                    Z: 0.0,
                })?;
                bar.SetCenterPoint(Vector3 {
                    X: 1.5,
                    Y: 14.0,
                    Z: 0.0,
                })?;
            }
            bar.SetScale(bar_scale(side, 0.3))?;
        }
        // 下次 set_equalizer 重新开始动画
        self.equalizer.set(None);
        Ok(())
    }

    pub fn set_equalizer(&self, playing: bool) -> Result<()> {
        if self.equalizer.replace(Some(playing)) == Some(playing) {
            return Ok(());
        }
        let side = self.edge.is_side();
        const PATTERNS: [[f32; 5]; 4] = [
            [0.35, 1.0, 0.55, 0.85, 0.35],
            [0.8, 0.4, 1.0, 0.5, 0.8],
            [0.5, 0.9, 0.35, 1.0, 0.5],
            [0.9, 0.55, 0.75, 0.4, 0.9],
        ];
        const DURATIONS: [f32; 4] = [900.0, 1100.0, 1000.0, 1250.0];
        for (i, bar) in self.bars.iter().enumerate() {
            if playing {
                let a = self.compositor.CreateVector3KeyFrameAnimation()?;
                for (k, y) in PATTERNS[i].iter().enumerate() {
                    a.InsertKeyFrame(k as f32 / 4.0, bar_scale(side, *y))?;
                }
                a.SetDuration(ms(DURATIONS[i]))?;
                a.SetIterationBehavior(AnimationIterationBehavior::Forever)?;
                bar.StartAnimation(&HSTRING::from("Scale"), &a)?;
            } else {
                bar.StopAnimation(&HSTRING::from("Scale"))?;
                bar.SetScale(bar_scale(side, 0.3))?;
            }
        }
        Ok(())
    }

    /// DPI 变化：整体缩放，内容表面按新的像素尺寸重建（调用方随后重画）。
    pub fn set_scale(&mut self, gfx: &Gfx, scale: f32) -> Result<()> {
        self.scale = scale;
        self.root.SetScale(Vector3 {
            X: scale,
            Y: scale,
            Z: 1.0,
        })?;
        for layer in [
            &mut self.compact,
            &mut self.level,
            &mut self.notification,
            &mut self.clock,
            &mut self.expanded,
        ] {
            layer.surface = gfx.surface(
                (layer.width * scale).ceil() as u32,
                (layer.height * scale).ceil() as u32,
            )?;
            let brush = self
                .compositor
                .CreateSurfaceBrushWithSurface(&layer.surface)?;
            brush.SetStretch(CompositionStretch::Fill)?;
            layer.visual.SetBrush(&brush)?;
        }
        Ok(())
    }
}

/// 律动条的缩放：顶部的竖条改变高度，侧边的横条改变长度。
fn bar_scale(side: bool, amount: f32) -> Vector3 {
    if side {
        Vector3 {
            X: amount,
            Y: 1.0,
            Z: 1.0,
        }
    } else {
        Vector3 {
            X: 1.0,
            Y: amount,
            Z: 1.0,
        }
    }
}

/// 内容块换尺寸：表面按新的像素尺寸重建。尺寸没变就什么都不做。
fn resize_layer(
    c: &Compositor,
    gfx: &Gfx,
    layer: &mut Layer,
    m: Metrics,
    scale: f32,
) -> Result<()> {
    if layer.width == m.width && layer.height == m.height {
        return Ok(());
    }
    layer.width = m.width;
    layer.height = m.height;
    layer.surface = gfx.surface(
        (m.width * scale).ceil() as u32,
        (m.height * scale).ceil() as u32,
    )?;
    let brush = c.CreateSurfaceBrushWithSurface(&layer.surface)?;
    brush.SetStretch(CompositionStretch::Fill)?;
    layer.visual.SetBrush(&brush)?;
    layer.visual.SetSize(Vector2 {
        X: m.width,
        Y: m.height,
    })
}

/// 描边宽度（DIP）。
const EDGE: f32 = 1.0;

/// 描边颜色：浅色胶囊用淡淡的黑边，深色胶囊用淡淡的白边。
fn edge_color(palette: &Palette) -> Color {
    if island::color::luminance(palette.background) > 0.45 {
        Color {
            A: 36,
            R: 0,
            G: 0,
            B: 0,
        }
    } else {
        Color {
            A: 28,
            R: 255,
            G: 255,
            B: 255,
        }
    }
}

/// 跟着属性集变化的圆角矩形；`grow` 是向外扩出的距离（描边用）。
fn rounded_geometry(
    c: &Compositor,
    props: &CompositionPropertySet,
    grow: f32,
) -> Result<CompositionRoundedRectangleGeometry> {
    let g = c.CreateRoundedRectangleGeometry()?;
    bind(
        c,
        &g,
        "Size",
        "Vector2(p.W + 2 * G, p.H + 2 * G)",
        props,
        &[("G", grow)],
    )?;
    bind(
        c,
        &g,
        "CornerRadius",
        "Vector2(p.Radius + G, p.Radius + G)",
        props,
        &[("G", grow)],
    )?;
    Ok(g)
}

fn make_layer(c: &Compositor, gfx: &Gfx, m: Metrics, scale: f32) -> Result<Layer> {
    let surface = gfx.surface(
        (m.width * scale).ceil() as u32,
        (m.height * scale).ceil() as u32,
    )?;
    let brush = c.CreateSurfaceBrushWithSurface(&surface)?;
    brush.SetStretch(CompositionStretch::Fill)?;
    let visual = c.CreateSpriteVisual()?;
    visual.SetBrush(&brush)?;
    visual.SetSize(Vector2 {
        X: m.width,
        Y: m.height,
    })?;
    visual.SetOpacity(0.0)?;
    // 位置和缩放中心由 Scene::set_edge 按停靠边设置
    Ok(Layer {
        visual,
        surface,
        width: m.width,
        height: m.height,
    })
}

/// 用表达式把某个属性绑定到属性集 p 上。
fn bind<T: Interface>(
    c: &Compositor,
    target: &T,
    property: &str,
    expression: &str,
    props: &CompositionPropertySet,
    scalars: &[(&str, f32)],
) -> Result<()> {
    let e = c.CreateExpressionAnimationWithExpression(&HSTRING::from(expression))?;
    e.SetReferenceParameter(&HSTRING::from("p"), props)?;
    for (name, value) in scalars {
        e.SetScalarParameter(&HSTRING::from(*name), *value)?;
    }
    let object: CompositionObject = target.cast()?;
    object.StartAnimation(&HSTRING::from(property), &e)
}

fn fade(c: &Compositor, visual: &SpriteVisual, visible: bool, growing: bool) -> Result<()> {
    let opacity = c.CreateScalarKeyFrameAnimation()?;
    opacity.InsertKeyFrame(1.0, if visible { 1.0 } else { 0.0 })?;
    if visible {
        opacity.SetDuration(ms(200.0))?;
        opacity.SetDelayTime(ms(if growing { 90.0 } else { 60.0 }))?;
    } else {
        opacity.SetDuration(ms(if growing { 90.0 } else { 70.0 }))?;
    }
    visual.StartAnimation(&HSTRING::from("Opacity"), &opacity)?;

    if visible {
        // 从略小（展开）或略大（收起）缩放到原尺寸，先快后慢
        let from = if growing { 0.94 } else { 1.04 };
        let ease = c.CreateCubicBezierEasingFunction(
            Vector2 { X: 0.2, Y: 0.9 },
            Vector2 { X: 0.3, Y: 1.0 },
        )?;
        let zoom = c.CreateVector3KeyFrameAnimation()?;
        zoom.InsertKeyFrame(
            0.0,
            Vector3 {
                X: from,
                Y: from,
                Z: 1.0,
            },
        )?;
        zoom.InsertKeyFrameWithEasingFunction(
            1.0,
            Vector3 {
                X: 1.0,
                Y: 1.0,
                Z: 1.0,
            },
            &ease,
        )?;
        zoom.SetDuration(ms(320.0))?;
        zoom.SetDelayTime(ms(if growing { 60.0 } else { 30.0 }))?;
        zoom.SetDelayBehavior(AnimationDelayBehavior::SetInitialValueBeforeDelay)?;
        visual.StartAnimation(&HSTRING::from("Scale"), &zoom)?;
    }
    Ok(())
}

/// 弹簧动画的目标值要装箱成 IReference<f32>。
fn boxed(v: f32) -> Result<IReference<f32>> {
    PropertyValue::CreateSingle(v)?.cast()
}
