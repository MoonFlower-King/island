using Island.Core;
using Island.Services;

namespace Island.Tests;

public class LevelTests
{
    [Theory]
    [InlineData(0.50, 1, 0.52)]   // 向上一格 +2%
    [InlineData(0.50, -1, 0.48)]  // 向下一格 -2%
    [InlineData(0.37, 1, 0.38)]   // 先按方向对齐到 2% 的整数倍
    [InlineData(0.37, -1, 0.36)]
    [InlineData(0.48, -1, 0.46)]  // 浮点误差（0.48/0.02 = 23.999…）不会多走一格
    [InlineData(0.99, 3, 1.00)]   // 不超过 100%
    [InlineData(0.01, -2, 0.00)]  // 不低于 0
    [InlineData(0.50, 0, 0.50)]
    public void WheelStepsVolumeInTwoPercentNotches(double level, int notches, double expected) =>
        Assert.Equal(expected, VolumeService.StepVolume(level, notches), 6);

    [Theory]
    [InlineData(0.0, false, LevelGlyphs.Muted)]
    [InlineData(0.8, true, LevelGlyphs.Muted)]
    [InlineData(0.2, false, LevelGlyphs.VolumeLow)]
    [InlineData(0.5, false, LevelGlyphs.VolumeMedium)]
    [InlineData(0.9, false, LevelGlyphs.VolumeHigh)]
    public void PicksVolumeIconByLevel(double level, bool muted, string glyph) =>
        Assert.Equal(glyph, LevelGlyphs.ForVolume(level, muted));

    [Theory]
    [InlineData(0.426, false, "43%")]
    [InlineData(1.2, false, "100%")]
    [InlineData(0.5, true, "静音")]
    public void FormatsPercent(double level, bool muted, string text) =>
        Assert.Equal(text, LevelGlyphs.Percent(level, muted));
}
