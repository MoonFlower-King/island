using System.Windows;
using System.Windows.Media;
using Island.Core;
using Microsoft.Win32;

namespace Island.Views;

/// <summary>
/// 把配色写进窗口资源，界面通过 DynamicResource 引用。
///
/// WPF 会冻结样式和模板里用到的画刷，所以不能直接对画刷做动画。这里的做法是：
/// 每一帧算出过渡中的颜色，生成一套新的（冻结的）画刷，整体替换掉窗口里的主题字典。
/// 每帧只触发一次资源刷新，过渡只有 0.3 秒，平时完全不占资源。
/// </summary>
public sealed class ThemeManager
{
    private static readonly TimeSpan TransitionDuration = TimeSpan.FromMilliseconds(320);

    private Dictionary<string, Color> _shown;
    private Dictionary<string, Color> _from = new();
    private Dictionary<string, Color> _to = new();
    private ResourceDictionary? _host;
    private ResourceDictionary? _themeDictionary;
    private TimeSpan _transitionStart;
    private bool _transitioning;

    public ThemeManager()
    {
        Current = ThemePalette.Dark;
        _shown = ColorsFor(Current).ToDictionary(c => c.Key, c => c.Color);
    }

    public ThemePalette Current { get; private set; }

    /// <summary>配色变化（用于更新阴影、重新计算音乐主题色等）。</summary>
    public event Action<ThemePalette>? Changed;

    /// <summary>把主题字典挂到窗口资源上。要在窗口 InitializeComponent 之前调用。</summary>
    public void AttachTo(ResourceDictionary windowResources)
    {
        _host = windowResources;
        Write(_shown);
    }

    /// <summary>当前显示中的颜色（过渡期间是中间色）。</summary>
    public Color ColorOf(string key) => _shown.TryGetValue(key, out var color) ? color : default;

    public void Apply(ThemePalette palette, bool animate)
    {
        if (palette == Current) return;
        Current = palette;
        _to = ColorsFor(palette).ToDictionary(c => c.Key, c => c.Color);

        if (animate && _host is not null)
        {
            _from = new Dictionary<string, Color>(_shown);
            _transitionStart = TimeSpan.Zero;
            if (!_transitioning)
            {
                _transitioning = true;
                CompositionTarget.Rendering += OnRendering;
            }
        }
        else
        {
            StopTransition();
            Write(_to);
        }

        Changed?.Invoke(palette);
    }

    private void OnRendering(object? sender, EventArgs e)
    {
        if (e is not RenderingEventArgs args) return;
        if (_transitionStart == TimeSpan.Zero) _transitionStart = args.RenderingTime;

        double t = (args.RenderingTime - _transitionStart).TotalMilliseconds / TransitionDuration.TotalMilliseconds;
        if (t >= 1)
        {
            StopTransition();
            Write(_to);
            return;
        }

        double eased = t < 0.5 ? 4 * t * t * t : 1 - Math.Pow(-2 * t + 2, 3) / 2; // ease-in-out cubic
        Write(_to.ToDictionary(kv => kv.Key,
            kv => ColorMath.Mix(_from.TryGetValue(kv.Key, out var from) ? from : kv.Value, kv.Value, eased)));
    }

    private void StopTransition()
    {
        if (!_transitioning) return;
        _transitioning = false;
        CompositionTarget.Rendering -= OnRendering;
    }

    private void Write(Dictionary<string, Color> colors)
    {
        _shown = colors;
        if (_host is null) return;

        var dictionary = new ResourceDictionary();
        foreach (var (key, color) in colors)
        {
            var brush = new SolidColorBrush(color);
            brush.Freeze();
            dictionary[key] = brush;
        }

        // 整体替换：一次资源刷新，而不是每个颜色各刷新一次
        if (_themeDictionary is not null) _host.MergedDictionaries.Remove(_themeDictionary);
        _host.MergedDictionaries.Add(dictionary);
        _themeDictionary = dictionary;
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
