using System.Windows;
using System.Windows.Media;
using System.Windows.Media.Animation;
using Island.Core;
using Microsoft.Win32;

namespace Island.Views;

/// <summary>
/// 把配色写进窗口资源。界面通过 DynamicResource 引用这些画刷；
/// 换主题时直接给同一批画刷做颜色动画，所有界面一起平滑变色，不需要重建任何控件。
///
/// 注意画刷必须放在窗口（而不是 Application）的资源里：WPF 会自动冻结应用级资源里
/// 被访问过的 Freezable，冻结后的画刷不能再做动画或修改颜色。
/// </summary>
public sealed class ThemeManager
{
    private static readonly TimeSpan TransitionDuration = TimeSpan.FromMilliseconds(320);

    private readonly Dictionary<string, SolidColorBrush> _brushes = new();

    public ThemeManager()
    {
        foreach (var (key, color) in ColorsFor(ThemePalette.Dark))
            _brushes[key] = new SolidColorBrush(color); // 不冻结：之后要对它做动画
        Current = ThemePalette.Dark;
    }

    /// <summary>把主题画刷放进窗口资源。要在窗口 InitializeComponent 之前调用。</summary>
    public void AttachTo(ResourceDictionary windowResources)
    {
        foreach (var (key, brush) in _brushes) windowResources[key] = brush;
    }

    public Color ColorOf(string key) => _brushes.TryGetValue(key, out var brush) ? brush.Color : default;

    public ThemePalette Current { get; private set; }

    /// <summary>配色变化（用于更新阴影、重新计算音乐主题色等）。</summary>
    public event Action<ThemePalette>? Changed;

    public void Apply(ThemePalette palette, bool animate)
    {
        if (palette == Current) return;
        Current = palette;

        foreach (var (key, color) in ColorsFor(palette))
        {
            SolidColorBrush brush = _brushes[key];
            if (animate)
            {
                var transition = new ColorAnimation(color, TransitionDuration)
                {
                    EasingFunction = new CubicEase { EasingMode = EasingMode.EaseInOut },
                };
                brush.BeginAnimation(SolidColorBrush.ColorProperty, transition, HandoffBehavior.SnapshotAndReplace);
            }
            else
            {
                brush.BeginAnimation(SolidColorBrush.ColorProperty, null);
                brush.Color = color;
            }
        }

        Changed?.Invoke(palette);
    }

    /// <summary>从三种基础颜色推导出界面用到的全部颜色。</summary>
    public static IEnumerable<(string Key, Color Color)> ColorsFor(ThemePalette p)
    {
        Color fg = p.Foreground;
        yield return ("IslandBackground", p.Background);
        yield return ("IslandForeground", fg);
        yield return ("IslandSecondary", ColorMath.WithAlpha(fg, 0x99));
        yield return ("IslandTertiary", ColorMath.WithAlpha(fg, 0x66));
        yield return ("IslandSurface", ColorMath.WithAlpha(fg, 0x1A));
        yield return ("IslandHover", ColorMath.WithAlpha(fg, 0x1F));
        yield return ("IslandSurfaceHover", ColorMath.WithAlpha(fg, 0x26));
        yield return ("IslandSurfaceStrong", ColorMath.WithAlpha(fg, 0x33));
        // 浅色胶囊在浅色网页上需要一圈细边才看得出轮廓
        yield return ("IslandOutline", ColorMath.WithAlpha(fg, p.IsLight ? (byte)0x1F : (byte)0x00));
        yield return ("IslandAccent", p.Accent);
        yield return ("IslandOnAccent", ColorMath.BestTextOn(p.Accent));
        yield return ("IslandGreen", p.IsLight ? Color.FromRgb(0x24, 0xA1, 0x48) : Color.FromRgb(0x30, 0xD1, 0x58));
    }

    /// <summary>Windows“应用模式”是否为浅色（设置 → 个性化 → 颜色）。</summary>
    public static bool SystemPrefersLight()
    {
        try
        {
            using var key = Registry.CurrentUser.OpenSubKey(@"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
            return key?.GetValue("AppsUseLightTheme") is int value && value != 0;
        }
        catch
        {
            return false;
        }
    }
}
