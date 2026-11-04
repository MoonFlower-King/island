using System.Windows.Media;
using Island.Core;

namespace Island.Tests;

public class ThemeTests
{
    private static readonly DateTime Noon = new(2026, 6, 1, 12, 0, 0);
    private static readonly DateTime Midnight = new(2026, 6, 1, 0, 30, 0);

    [Theory]
    [InlineData(ThemeMode.Dark, false)]
    [InlineData(ThemeMode.Light, true)]
    public void FixedModesIgnoreTimeAndSystem(ThemeMode mode, bool light)
    {
        var settings = new ThemeSettings { Mode = mode };
        foreach (DateTime time in new[] { Noon, Midnight })
        foreach (bool system in new[] { true, false })
            Assert.Equal(light, ThemeResolver.Resolve(settings, time, system).IsLight);
    }

    [Fact]
    public void SystemModeFollowsWindows()
    {
        var settings = new ThemeSettings { Mode = ThemeMode.System };
        Assert.Equal(ThemePalette.Light, ThemeResolver.Resolve(settings, Midnight, systemPrefersLight: true));
        Assert.Equal(ThemePalette.Dark, ThemeResolver.Resolve(settings, Noon, systemPrefersLight: false));
    }

    [Fact]
    public void ScheduleModeSwitchesByTime()
    {
        var settings = new ThemeSettings { Mode = ThemeMode.Schedule, DayStarts = "07:00", NightStarts = "19:00" };
        Assert.Equal(ThemePalette.Light, ThemeResolver.Resolve(settings, Noon, false));
        Assert.Equal(ThemePalette.Dark, ThemeResolver.Resolve(settings, Midnight, true));
        Assert.True(ThemeResolver.IsDaytime(settings, new TimeSpan(7, 0, 0)));
        Assert.False(ThemeResolver.IsDaytime(settings, new TimeSpan(19, 0, 0)));
    }

    [Fact]
    public void ScheduleSupportsRangesAcrossMidnight()
    {
        var settings = new ThemeSettings { DayStarts = "22:00", NightStarts = "06:00" };
        Assert.True(ThemeResolver.IsDaytime(settings, new TimeSpan(23, 0, 0)));
        Assert.True(ThemeResolver.IsDaytime(settings, new TimeSpan(5, 59, 0)));
        Assert.False(ThemeResolver.IsDaytime(settings, new TimeSpan(12, 0, 0)));
    }

    [Fact]
    public void CustomModeUsesUserColorsAndFallsBackOnBadInput()
    {
        var settings = new ThemeSettings
        {
            Mode = ThemeMode.Custom, CustomBackground = "#0B1F3A", CustomForeground = "fff", CustomAccent = "oops",
        };
        ThemePalette p = ThemeResolver.Resolve(settings, Noon, false);
        Assert.Equal(Color.FromRgb(0x0B, 0x1F, 0x3A), p.Background);
        Assert.Equal(Colors.White, p.Foreground);
        Assert.Equal(ThemePalette.Dark.Accent, p.Accent);
    }

    [Theory]
    [InlineData("#1C1C1E", true)]
    [InlineData("1c1c1e", true)]
    [InlineData(" #abc ", true)]
    [InlineData("#12345", false)]
    [InlineData("#GGGGGG", false)]
    [InlineData("", false)]
    public void ParsesHexColors(string text, bool valid) => Assert.Equal(valid, ColorMath.TryParseHex(text, out _));

    [Fact]
    public void ContrastRatioMatchesWcagExtremes()
    {
        Assert.Equal(21, ColorMath.ContrastRatio(Colors.Black, Colors.White), 1);
        Assert.Equal(1, ColorMath.ContrastRatio(Colors.Gray, Colors.Gray), 3);
    }

    [Theory]
    [InlineData(0x00, 0x00, 0x00)] // 黑夜
    [InlineData(0xF7, 0xF7, 0xF8)] // 白天
    [InlineData(0x0B, 0x1F, 0x3A)] // 深蓝
    public void AccentIsMadeReadableOnAnyBackground(byte r, byte g, byte b)
    {
        Color background = Color.FromRgb(r, g, b);
        foreach (Color raw in new[] { Color.FromRgb(20, 30, 90), Color.FromRgb(255, 240, 200), Color.FromRgb(48, 209, 88) })
        {
            Color readable = ColorMath.EnsureReadable(raw, background);
            Assert.True(ColorMath.ContrastRatio(readable, background) >= 2.9,
                $"{ColorMath.ToHex(readable)} 在 {ColorMath.ToHex(background)} 上看不清");
        }
    }

    [Fact]
    public void ThemeColorsAreDerivedForEveryResourceKey()
    {
        var keys = Island.Views.ThemeManager.ColorsFor(ThemePalette.Light).Select(c => c.Key).ToList();
        Assert.Contains("IslandBackground", keys);
        Assert.Contains("IslandOnAccent", keys);
        Assert.Equal(keys.Count, keys.Distinct().Count());
    }
}
