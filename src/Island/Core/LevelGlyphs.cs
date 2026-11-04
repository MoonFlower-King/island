namespace Island.Core;

/// <summary>音量 / 亮度提示用的图标（Segoe Fluent Icons 字形）和文字。</summary>
public static class LevelGlyphs
{
    public const string Muted = "";
    public const string VolumeLow = "";
    public const string VolumeMedium = "";
    public const string VolumeHigh = "";
    public const string Brightness = "";

    public static string ForVolume(double level, bool muted) =>
        muted || level <= 0.001 ? Muted
        : level < 0.34 ? VolumeLow
        : level < 0.67 ? VolumeMedium
        : VolumeHigh;

    public static string Percent(double level, bool muted) =>
        muted ? "静音" : $"{Math.Round(Math.Clamp(level, 0, 1) * 100):0}%";
}
