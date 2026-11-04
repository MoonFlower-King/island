//! 图形设备：D3D11 + Direct2D + DirectWrite，以及 Windows 合成器（Composition）用的绘图设备。
//!
//! 岛的形状、阴影和所有动画都交给系统合成器，在独立的合成线程上逐帧运行，不占界面线程；
//! Direct2D 只在内容变化时（换歌、整分钟、鼠标移到按钮上）把文字和图标画进一张合成表面。

use std::cell::RefCell;
use windows::Foundation::Size;
use windows::Graphics::DirectX::{DirectXAlphaMode, DirectXPixelFormat};
use windows::UI::Composition::{CompositionDrawingSurface, CompositionGraphicsDevice, Compositor};
use windows::Win32::Foundation::{HMODULE, POINT};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::System::WinRT::Composition::{
    ICompositionDrawingSurfaceInterop, ICompositorInterop,
};

use windows::core::{HSTRING, Interface, Result, w};
use windows_numerics::Matrix3x2;

use island::color::Rgb;
use island::metrics::Rect;

pub struct Gfx {
    pub dwrite: IDWriteFactory,
    pub device: CompositionGraphicsDevice,
    /// 图标字体：Windows 11 上是 Segoe Fluent Icons，Windows 10 上是 Segoe MDL2 Assets。
    pub icon_font: HSTRING,
    /// 文字字体：Windows 11 上是 Segoe UI Variable，Windows 10 上是 Segoe UI。中文由 DirectWrite 自动回退到微软雅黑。
    pub text_font: HSTRING,
    /// 显卡是微软的软件渲染器（没有显卡的虚拟机、远程桌面）。这时一些合成效果（阴影）不可靠。
    pub software: bool,
    /// 文字格式缓存：界面上只有十几种字号 / 字重组合，不必每画一段文字就新建一次。
    formats: RefCell<Vec<(FormatKey, IDWriteTextFormat)>>,
    /// 共用的纯色画刷，每次填充前改颜色。D2D 资源属于设备，可以跨多次绘制复用。
    brush: RefCell<Option<ID2D1SolidColorBrush>>,
    /// 最近一张封面的位图（按封面编号），播放中每秒重画进度时不用重新上传像素。
    image: RefCell<Option<(u64, ID2D1Bitmap1)>>,
}

#[derive(Clone, PartialEq)]
struct FormatKey {
    family: HSTRING,
    size_bits: u32,
    weight: i32,
    align: Align,
}

impl Gfx {
    pub fn new(compositor: &Compositor) -> Result<Self> {
        let d3d = create_d3d_device()?;
        let dxgi: IDXGIDevice = d3d.cast()?;
        // Microsoft Basic Render Driver / WARP 的厂商号是 0x1414
        let software = unsafe { dxgi.GetAdapter().and_then(|a| a.GetDesc()) }
            .map(|d| d.VendorId == 0x1414)
            .unwrap_or(false);
        let factory: ID2D1Factory1 =
            unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)? };
        let d2d = unsafe { factory.CreateDevice(&dxgi)? };
        let interop: ICompositorInterop = compositor.cast()?;
        let device = unsafe { interop.CreateGraphicsDevice(&d2d)? };
        let dwrite: IDWriteFactory = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)? };
        let icon_font = if font_exists(&dwrite, "Segoe Fluent Icons") {
            HSTRING::from("Segoe Fluent Icons")
        } else {
            HSTRING::from("Segoe MDL2 Assets")
        };
        let text_font = if font_exists(&dwrite, "Segoe UI Variable Display") {
            HSTRING::from("Segoe UI Variable Display")
        } else {
            HSTRING::from("Segoe UI")
        };
        Ok(Self {
            dwrite,
            device,
            icon_font,
            text_font,
            software,
            formats: RefCell::new(Vec::new()),
            brush: RefCell::new(None),
            image: RefCell::new(None),
        })
    }

    /// 一张透明的合成表面，按物理像素分配。
    pub fn surface(&self, width_px: u32, height_px: u32) -> Result<CompositionDrawingSurface> {
        self.device.CreateDrawingSurface(
            Size {
                Width: width_px.max(1) as f32,
                Height: height_px.max(1) as f32,
            },
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            DirectXAlphaMode::Premultiplied,
        )
    }

    /// 在表面上作画。`scale` 是 DPI 缩放，回调里一律使用 DIP 坐标。
    pub fn draw(
        &self,
        surface: &CompositionDrawingSurface,
        scale: f32,
        paint: impl FnOnce(&Painter) -> Result<()>,
    ) -> Result<()> {
        let interop: ICompositionDrawingSurfaceInterop = surface.cast()?;
        let mut offset = POINT::default();
        let dc: ID2D1DeviceContext = unsafe { interop.BeginDraw(None, &mut offset)? };
        unsafe {
            dc.SetDpi(96.0 * scale, 96.0 * scale);
            dc.SetTransform(&Matrix3x2::translation(
                offset.x as f32 / scale,
                offset.y as f32 / scale,
            ));
            dc.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            dc.Clear(Some(&D2D1_COLOR_F {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            }));
        }
        let result = paint(&Painter { dc: &dc, gfx: self });
        // 无论画没画成都要结束绘制，否则表面一直被锁着
        unsafe { interop.EndDraw()? };
        result
    }
}

fn create_d3d_device() -> Result<ID3D11Device> {
    let mut last = None;
    for driver in [D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP] {
        let mut device = None;
        match unsafe {
            D3D11CreateDevice(
                None,
                driver,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                None,
            )
        } {
            Ok(()) => return Ok(device.expect("D3D11CreateDevice 成功但没有返回设备")),
            Err(e) => last = Some(e),
        }
    }
    Err(last.expect("至少尝试过一种驱动"))
}

fn font_exists(dwrite: &IDWriteFactory, family: &str) -> bool {
    unsafe {
        let mut collection = None;
        if dwrite
            .GetSystemFontCollection(&mut collection, false)
            .is_err()
        {
            return false;
        }
        let Some(collection) = collection else {
            return false;
        };
        let (mut index, mut exists) = (0u32, windows::core::BOOL::default());
        collection
            .FindFamilyName(&HSTRING::from(family), &mut index, &mut exists)
            .is_ok()
            && exists.as_bool()
    }
}

// ───────────────────────────── 画笔 ─────────────────────────────

/// 文字对齐。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Leading,
    Center,
    Trailing,
}

pub struct TextStyle<'a> {
    pub family: &'a HSTRING,
    pub size: f32,
    pub weight: DWRITE_FONT_WEIGHT,
    pub align: Align,
}

/// 对 ID2D1DeviceContext 的一层薄封装，坐标都是 DIP。
pub struct Painter<'a> {
    pub dc: &'a ID2D1DeviceContext,
    pub gfx: &'a Gfx,
}

pub fn color(c: Rgb, alpha: f32) -> D2D1_COLOR_F {
    let [r, g, b] = c.to_f32();
    D2D1_COLOR_F { r, g, b, a: alpha }
}

fn rect_f(r: Rect) -> D2D_RECT_F {
    D2D_RECT_F {
        left: r.x,
        top: r.y,
        right: r.right(),
        bottom: r.bottom(),
    }
}

impl Painter<'_> {
    pub fn brush(&self, c: D2D1_COLOR_F) -> Result<ID2D1SolidColorBrush> {
        let mut cached = self.gfx.brush.borrow_mut();
        if let Some(b) = cached.as_ref() {
            unsafe { b.SetColor(&c) };
            return Ok(b.clone());
        }
        let b = unsafe { self.dc.CreateSolidColorBrush(&c, None)? };
        *cached = Some(b.clone());
        Ok(b)
    }

    fn text_format(&self, style: &TextStyle) -> Result<IDWriteTextFormat> {
        let key = FormatKey {
            family: style.family.clone(),
            size_bits: style.size.to_bits(),
            weight: style.weight.0,
            align: style.align,
        };
        if let Some((_, f)) = self.gfx.formats.borrow().iter().find(|(k, _)| *k == key) {
            return Ok(f.clone());
        }
        let format = unsafe {
            let format = self.gfx.dwrite.CreateTextFormat(
                style.family,
                None,
                style.weight,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                style.size,
                w!("zh-CN"),
            )?;
            format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            format.SetTextAlignment(match style.align {
                Align::Leading => DWRITE_TEXT_ALIGNMENT_LEADING,
                Align::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
                Align::Trailing => DWRITE_TEXT_ALIGNMENT_TRAILING,
            })?;
            let trimming = DWRITE_TRIMMING {
                granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                delimiter: 0,
                delimiterCount: 0,
            };
            let sign = self.gfx.dwrite.CreateEllipsisTrimmingSign(&format)?;
            format.SetTrimming(&trimming, &sign)?;
            format
        };
        self.gfx.formats.borrow_mut().push((key, format.clone()));
        Ok(format)
    }

    pub fn fill_round(&self, r: Rect, radius: f32, c: D2D1_COLOR_F) -> Result<()> {
        let brush = self.brush(c)?;
        unsafe {
            self.dc.FillRoundedRectangle(
                &D2D1_ROUNDED_RECT {
                    rect: rect_f(r),
                    radiusX: radius,
                    radiusY: radius,
                },
                &brush,
            )
        };
        Ok(())
    }

    pub fn fill_circle(&self, cx: f32, cy: f32, radius: f32, c: D2D1_COLOR_F) -> Result<()> {
        let brush = self.brush(c)?;
        unsafe {
            self.dc.FillEllipse(
                &D2D1_ELLIPSE {
                    point: windows_numerics::Vector2 { X: cx, Y: cy },
                    radiusX: radius,
                    radiusY: radius,
                },
                &brush,
            )
        };
        Ok(())
    }

    /// 单行文字，放不下时末尾显示省略号，在 `r` 里垂直居中。
    pub fn text(&self, s: &str, r: Rect, style: &TextStyle, c: D2D1_COLOR_F) -> Result<()> {
        if s.is_empty() {
            return Ok(());
        }
        let format = self.text_format(style)?;
        unsafe {
            let wide: Vec<u16> = s.encode_utf16().collect();
            let brush = self.brush(c)?;
            self.dc.DrawText(
                &wide,
                &format,
                &rect_f(r),
                &brush,
                D2D1_DRAW_TEXT_OPTIONS_CLIP | D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
        Ok(())
    }

    /// 图标字体里的一个字形，居中画在 `r` 里。
    pub fn icon(&self, glyph: char, r: Rect, size: f32, c: D2D1_COLOR_F) -> Result<()> {
        let style = TextStyle {
            family: &self.gfx.icon_font,
            size,
            weight: DWRITE_FONT_WEIGHT_NORMAL,
            align: Align::Center,
        };
        self.text(&glyph.to_string(), r, &style, c)
    }

    /// 把 BGRA（预乘）像素画成圆角图片。`id` 相同的图片只上传一次。
    pub fn image(
        &self,
        id: u64,
        pixels: &[u8],
        width: u32,
        height: u32,
        r: Rect,
        radius: f32,
    ) -> Result<()> {
        unsafe {
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                bitmapOptions: D2D1_BITMAP_OPTIONS_NONE,
                colorContext: std::mem::ManuallyDrop::new(None),
            };
            let cached = self
                .gfx
                .image
                .borrow()
                .as_ref()
                .filter(|(k, _)| *k == id)
                .map(|(_, b)| b.clone());
            let bitmap = match cached {
                Some(b) => b,
                None => {
                    let b = self.dc.CreateBitmap(
                        D2D_SIZE_U { width, height },
                        Some(pixels.as_ptr().cast()),
                        width * 4,
                        &props,
                    )?;
                    *self.gfx.image.borrow_mut() = Some((id, b.clone()));
                    b
                }
            };
            let brush_props = D2D1_BITMAP_BRUSH_PROPERTIES1 {
                extendModeX: D2D1_EXTEND_MODE_CLAMP,
                extendModeY: D2D1_EXTEND_MODE_CLAMP,
                interpolationMode: D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC,
            };
            let brush = self
                .dc
                .CreateBitmapBrush(&bitmap, Some(&brush_props), None)?;
            // 位图按 96 DPI 创建，宽高就是 DIP；缩放到目标矩形
            let m = Matrix3x2 {
                M11: r.w / width as f32,
                M12: 0.0,
                M21: 0.0,
                M22: r.h / height as f32,
                M31: r.x,
                M32: r.y,
            };
            brush.SetTransform(&m);
            self.dc.FillRoundedRectangle(
                &D2D1_ROUNDED_RECT {
                    rect: rect_f(r),
                    radiusX: radius,
                    radiusY: radius,
                },
                &brush,
            );
        }
        Ok(())
    }
}
