//! 合成树：胶囊形状、阴影、裁剪和所有动画都在这里，交给系统合成器在独立线程上运行。
//!
//! ```text
//! root（DPI 缩放）
//!  └ capsule（位置、大小跟着属性集 p 走）
//!     ├ layer（LayerVisual，带 DropShadow：阴影跟着形状实时变化）
//!     │   └ background（纯色，用圆角矩形裁剪）
//!     └ content（用同样的圆角矩形裁剪）
//!         ├ compact（音乐胶囊：封面、歌名、律动条）
//!         └ expanded（展开面板）
//! ```
//!
//! 形状只由属性集 p 的 Size / Radius / Top 三个值决定，状态切换时对它们启动弹簧动画，
//! 其余视觉元素用表达式动画绑定到 p 上。整个过程界面线程不参与逐帧计算。

use windows::Foundation::{IReference, PropertyValue, TimeSpan};
use windows::UI::Color;
use windows::UI::Composition::*;
use windows::core::{HSTRING, Interface, Result};
use windows_numerics::{Vector2, Vector3};

use island::color::{Palette, Rgb};
use island::metrics::{self, Metrics, WINDOW_HEIGHT, WINDOW_WIDTH};

use super::gfx::Gfx;

/// 弹簧参数：周期（秒）和阻尼比。展开时带一点回弹，收起时干脆利落。
#[derive(Clone, Copy)]
pub struct Spring {
    pub period: f32,
    pub damping: f32,
}

pub const EXPAND: Spring = Spring {
    period: 0.40,
    damping: 0.72,
};
pub const COLLAPSE: Spring = Spring {
    period: 0.30,
    damping: 0.92,
};

/// 一块固定尺寸的内容：画在合成表面上，按最终尺寸摆放一次，顶部居中对齐胶囊。
/// 胶囊动画时只是裁出它的可见部分，内容本身不需要重新布局或重画。
pub struct Layer {
    pub visual: SpriteVisual,
    pub surface: CompositionDrawingSurface,
    pub width: f32,
    pub height: f32,
}

pub struct Scene {
    pub compositor: Compositor,
    pub root: ContainerVisual,
    props: CompositionPropertySet,
    bg_brush: CompositionColorBrush,
    shadow: DropShadow,
    layer: LayerVisual,
    pub compact: Layer,
    pub expanded: Layer,
    bars: Vec<SpriteVisual>,
    bar_brush: CompositionColorBrush,
    scale: f32,
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
        bind(
            c,
            &capsule,
            "Offset",
            "Vector3((WW - p.W) / 2, p.Top, 0)",
            &props,
            &[("WW", WINDOW_WIDTH)],
        )?;
        root.Children()?.InsertAtTop(&capsule)?;

        // 背景形状 + 阴影
        // 背景：纯色 SpriteVisual，用圆角矩形裁剪。
        // （ShapeVisual 放在 LayerVisual 里时，填充色改了不生效，所以不用它。）
        let bg_brush = c.CreateColorBrushWithColor(ui_color(palette.background, 255))?;
        let background = c.CreateSpriteVisual()?;
        background.SetBrush(&bg_brush)?;
        bind(c, &background, "Size", "Vector2(p.W, p.H)", &props, &[])?;
        background.SetClip(&c.CreateGeometricClipWithGeometry(&rounded_geometry(c, &props)?)?)?;

        let layer = c.CreateLayerVisual()?;
        bind(c, &layer, "Size", "Vector2(p.W, p.H)", &props, &[])?;
        let shadow = c.CreateDropShadow()?;
        shadow.SetBlurRadius(18.0)?;
        shadow.SetOffset(Vector3 {
            X: 0.0,
            Y: 3.0,
            Z: 0.0,
        })?;
        shadow.SetColor(Color {
            A: 255,
            R: 0,
            G: 0,
            B: 0,
        })?;
        shadow.SetOpacity(0.45)?;
        layer.SetShadow(&shadow)?;
        layer.Children()?.InsertAtTop(&background)?;
        capsule.Children()?.InsertAtTop(&layer)?;

        // 内容：用同样的形状裁剪
        let content = c.CreateContainerVisual()?;
        bind(c, &content, "Size", "Vector2(p.W, p.H)", &props, &[])?;
        content.SetClip(&c.CreateGeometricClipWithGeometry(&rounded_geometry(c, &props)?)?)?;
        capsule.Children()?.InsertAtTop(&content)?;

        let compact = make_layer(c, gfx, &props, metrics::COMPACT, scale)?;
        let expanded = make_layer(c, gfx, &props, metrics::EXPANDED, scale)?;
        content.Children()?.InsertAtTop(&compact.visual)?;
        content.Children()?.InsertAtTop(&expanded.visual)?;

        // 律动条：4 根，挂在音乐胶囊右侧，动画全在合成器里跑
        let bar_brush = c.CreateColorBrushWithColor(ui_color(palette.accent, 255))?;
        let mut bars = Vec::new();
        for i in 0..4 {
            let bar = c.CreateSpriteVisual()?;
            bar.SetBrush(&bar_brush)?;
            bar.SetSize(Vector2 { X: 3.0, Y: 14.0 })?;
            bar.SetOffset(Vector3 {
                X: 222.0 + i as f32 * 6.0,
                Y: 12.0,
                Z: 0.0,
            })?;
            bar.SetCenterPoint(Vector3 {
                X: 1.5,
                Y: 14.0,
                Z: 0.0,
            })?;
            bar.SetScale(Vector3 {
                X: 1.0,
                Y: 0.3,
                Z: 1.0,
            })?;
            compact.visual.Children()?.InsertAtTop(&bar)?;
            bars.push(bar);
        }

        Ok(Self {
            compositor: c.clone(),
            root,
            props,
            bg_brush,
            layer: layer.clone(),
            shadow,
            compact,
            expanded,
            bars,
            bar_brush,
            scale,
        })
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
    pub fn show(&self, compact: bool, expanded: bool, growing: bool) -> Result<()> {
        for (layer, visible) in [(&self.compact, compact), (&self.expanded, expanded)] {
            fade(&self.compositor, &layer.visual, visible, growing)?;
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
        // 浅色背景上阴影淡一些
        let light = island::color::luminance(palette.background) > 0.45;
        self.shadow.SetOpacity(if light { 0.18 } else { 0.45 })?;
        Ok(())
    }

    /// 去掉阴影（没有显卡、合成器跑在软件渲染上时，阴影效果可能画不出来）。
    pub fn disable_shadow(&self) -> Result<()> {
        self.layer.SetShadow(None::<&CompositionShadow>)
    }

    /// 诊断：在窗口左下角放一块白色方块（不经过阴影层），确认纯色画刷能正常显示。
    pub fn add_probe(&self) -> Result<()> {
        let probe = self.compositor.CreateSpriteVisual()?;
        probe.SetBrush(&self.compositor.CreateColorBrushWithColor(Color {
            A: 255,
            R: 255,
            G: 255,
            B: 255,
        })?)?;
        probe.SetSize(Vector2 { X: 30.0, Y: 30.0 })?;
        probe.SetOffset(Vector3 {
            X: 0.0,
            Y: 240.0,
            Z: 0.0,
        })?;
        self.root.Children()?.InsertAtTop(&probe)
    }

    pub fn set_accent(&self, accent: Rgb) -> Result<()> {
        self.bar_brush.SetColor(ui_color(accent, 255))
    }

    /// 律动条：播放时随机高度来回跳，暂停时缩成短条。
    pub fn set_equalizer(&self, playing: bool) -> Result<()> {
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
                    a.InsertKeyFrame(
                        k as f32 / 4.0,
                        Vector3 {
                            X: 1.0,
                            Y: *y,
                            Z: 1.0,
                        },
                    )?;
                }
                a.SetDuration(ms(DURATIONS[i]))?;
                a.SetIterationBehavior(AnimationIterationBehavior::Forever)?;
                bar.StartAnimation(&HSTRING::from("Scale"), &a)?;
            } else {
                bar.StopAnimation(&HSTRING::from("Scale"))?;
                bar.SetScale(Vector3 {
                    X: 1.0,
                    Y: 0.3,
                    Z: 1.0,
                })?;
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
        for layer in [&mut self.compact, &mut self.expanded] {
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

fn rounded_geometry(
    c: &Compositor,
    props: &CompositionPropertySet,
) -> Result<CompositionRoundedRectangleGeometry> {
    let g = c.CreateRoundedRectangleGeometry()?;
    bind(c, &g, "Size", "Vector2(p.W, p.H)", props, &[])?;
    bind(
        c,
        &g,
        "CornerRadius",
        "Vector2(p.Radius, p.Radius)",
        props,
        &[],
    )?;
    Ok(g)
}

fn make_layer(
    c: &Compositor,
    gfx: &Gfx,
    props: &CompositionPropertySet,
    m: Metrics,
    scale: f32,
) -> Result<Layer> {
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
    visual.SetCenterPoint(Vector3 {
        X: m.width / 2.0,
        Y: 0.0,
        Z: 0.0,
    })?;
    bind(
        c,
        &visual,
        "Offset",
        "Vector3((p.W - VW) / 2, 0, 0)",
        props,
        &[("VW", m.width)],
    )?;
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
