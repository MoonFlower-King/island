using System.Globalization;
using System.Text.Json.Serialization;
using System.Windows.Media;

namespace Island.Core;

/// <summary>主题模式。</summary>
[JsonConverter(typeof(JsonStringEnumConverter<ThemeMode>))]
public enum ThemeMode
{
    /// <summary>黑夜：经典的黑色灵动岛。</summary>
    Dark,

    /// <summary>白天：浅色胶囊、深色文字。</summary>
    Light,

    /// <summary>跟随 Windows 的“应用模式”（设置 → 个性化 → 颜色）。</summary>
    System,

    /// <summary>按时间：白天时段用浅色，其余时间用深色。</summary>
    Schedule,

    /// <summary>自定义背景、文字和强调色。</summary>
    Custom,
}

/// <summary>主题的三种基础颜色，其他颜色（次要文字、悬停底色等）都由它们推导。</summary>
public readonly record struct ThemePalette(Color Background, Color Foreground, Color Accent)
{
    public static ThemePalette Dark { get; } =
        new(Color.FromRgb(0x00, 0x00, 0x00), Color.FromRgb(0xFF, 0xFF, 0xFF), Color.FromRgb(0x0A, 0x84, 0xFF));

    public static ThemePalette Light { get; } =
        new(Color.FromRgb(0xF7, 0xF7, 0xF8), Color.FromRgb(0x1C, 0x1C, 0x1E), Color.FromRgb(0x00, 0x7A, 0xFF));

    /// <summary>背景是浅色（影响阴影、描边，以及音乐主题色往深还是往浅调）。</summary>
    public bool IsLight => ColorMath.Luminance(Background) > 0.45;
}

/// <summary>主题相关的设置，保存在 settings.json 的 Theme 字段里。</summary>
public sealed class ThemeSettings
{
    public ThemeMode Mode { get; set; } = ThemeMode.Dark;

    /// <summary>“按时间”模式下白天开始的时刻（HH:mm）。</summary>
    public string DayStarts { get; set; } = "07:00";

    /// <summary>“按时间”模式下黑夜开始的时刻（HH:mm）。</summary>
    public string NightStarts { get; set; } = "19:00";

    public string CustomBackground { get; set; } = "#1C1C1E";
    public string CustomForeground { get; set; } = "#FFFFFF";
    public string CustomAccent { get; set; } = "#FF9F0A";
}

/// <summary>根据设置、当前时间和系统偏好，算出此刻应该用的配色。</summary>
public static class ThemeResolver
{
    public static ThemePalette Resolve(ThemeSettings settings, DateTime now, bool systemPrefersLight) =>
        settings.Mode switch
        {
            ThemeMode.Light => ThemePalette.Light,
            ThemeMode.System => systemPrefersLight ? ThemePalette.Light : ThemePalette.Dark,
            ThemeMode.Schedule => IsDaytime(settings, now.TimeOfDay) ? ThemePalette.Light : ThemePalette.Dark,
            ThemeMode.Custom => Custom(settings),
            _ => ThemePalette.Dark,
        };

    /// <summary>判断是否处在白天时段，支持跨午夜（例如白天 20:00 到 次日 06:00 这种反常设置）。</summary>
    public static bool IsDaytime(ThemeSettings settings, TimeSpan timeOfDay)
    {
        TimeSpan day = ParseTime(settings.DayStarts, new TimeSpan(7, 0, 0));
        TimeSpan night = ParseTime(settings.NightStarts, new TimeSpan(19, 0, 0));
        if (day == night) return true;
        return day < night
            ? timeOfDay >= day && timeOfDay < night
            : timeOfDay >= day || timeOfDay < night;
    }

    public static bool TryParseTime(string? text, out TimeSpan time) =>
        TimeSpan.TryParseExact(text?.Trim(), [@"h\:mm", @"hh\:mm"], CultureInfo.InvariantCulture, out time) &&
        time >= TimeSpan.Zero && time < TimeSpan.FromDays(1);

    private static TimeSpan ParseTime(string? text, TimeSpan fallback) =>
        TryParseTime(text, out TimeSpan t) ? t : fallback;

    private static ThemePalette Custom(ThemeSettings s)
    {
        Color background = ColorMath.TryParseHex(s.CustomBackground, out var bg) ? bg : ThemePalette.Dark.Background;
        Color foreground = ColorMath.TryParseHex(s.CustomForeground, out var fg) ? fg : ColorMath.BestTextOn(background);
        Color accent = ColorMath.TryParseHex(s.CustomAccent, out var ac) ? ac : ThemePalette.Dark.Accent;
        return new ThemePalette(background, foreground, accent);
    }
}

/// <summary>颜色计算：解析、亮度、对比度（按 WCAG 的定义）。</summary>
public static class ColorMath
{
    public static bool TryParseHex(string? text, out Color color)
    {
        color = default;
        if (string.IsNullOrWhiteSpace(text)) return false;

        string hex = text.Trim().TrimStart('#');
        if (hex.Length == 3) hex = string.Concat(hex.Select(c => $"{c}{c}"));
        if (hex.Length != 6 || !uint.TryParse(hex, NumberStyles.HexNumber, CultureInfo.InvariantCulture, out uint v))
            return false;

        color = Color.FromRgb((byte)(v >> 16), (byte)(v >> 8), (byte)v);
        return true;
    }

    public static string ToHex(Color c) => $"#{c.R:X2}{c.G:X2}{c.B:X2}";

    public static Color WithAlpha(Color c, byte alpha) => Color.FromArgb(alpha, c.R, c.G, c.B);

    /// <summary>相对亮度（0 = 黑，1 = 白）。</summary>
    public static double Luminance(Color c)
    {
        static double Channel(byte v)
        {
            double s = v / 255.0;
            return s <= 0.03928 ? s / 12.92 : Math.Pow((s + 0.055) / 1.055, 2.4);
        }
        return 0.2126 * Channel(c.R) + 0.7152 * Channel(c.G) + 0.0722 * Channel(c.B);
    }

    /// <summary>对比度，1（完全一样）到 21（黑白）。正文一般要求至少 4.5。</summary>
    public static double ContrastRatio(Color a, Color b)
    {
        double la = Luminance(a), lb = Luminance(b);
        return (Math.Max(la, lb) + 0.05) / (Math.Min(la, lb) + 0.05);
    }

    /// <summary>在这个背景上黑字和白字哪个更清楚。</summary>
    public static Color BestTextOn(Color background) =>
        ContrastRatio(Colors.White, background) >= ContrastRatio(Color.FromRgb(0x1C, 0x1C, 0x1E), background)
            ? Colors.White
            : Color.FromRgb(0x1C, 0x1C, 0x1E);

    public static Color Mix(Color from, Color to, double t)
    {
        t = Math.Clamp(t, 0, 1);
        return Color.FromArgb(
            (byte)Math.Round(from.A + (to.A - from.A) * t),
            (byte)Math.Round(from.R + (to.R - from.R) * t),
            (byte)Math.Round(from.G + (to.G - from.G) * t),
            (byte)Math.Round(from.B + (to.B - from.B) * t));
    }

    /// <summary>
    /// 把一个颜色（例如从封面取的主题色）调到在背景上看得清：深色背景往亮调，浅色背景往暗调。
    /// 已经够清楚的颜色原样返回，尽量保留原本的色相。
    /// </summary>
    public static Color EnsureReadable(Color color, Color background, double minimumContrast = 3)
    {
        color = Color.FromRgb(color.R, color.G, color.B);
        if (ContrastRatio(color, background) >= minimumContrast) return color;

        bool darkBackground = Luminance(background) < 0.45;
        if (darkBackground)
        {
            // 先按比例提亮保持饱和度，不够再往白色混
            byte max = Math.Max(color.R, Math.Max(color.G, color.B));
            if (max > 0 && max < 217)
            {
                double boost = 217.0 / max;
                color = Color.FromRgb(Scale(color.R, boost), Scale(color.G, boost), Scale(color.B, boost));
            }
        }

        Color toward = darkBackground ? Colors.White : Colors.Black;
        for (int step = 1; step <= 20 && ContrastRatio(color, background) < minimumContrast; step++)
            color = Mix(color, toward, 0.08);
        return color;
    }

    private static byte Scale(byte v, double factor) => (byte)Math.Clamp(Math.Round(v * factor), 0, 255);
}
