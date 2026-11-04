using Island.Animation;

namespace Island.Tests;

public class SpringEaseTests
{
    [Theory]
    [InlineData(0.66, 11)]
    [InlineData(0.92, 12)]
    [InlineData(0.8, 10)]
    public void StartsAtZeroAndEndsExactlyAtOne(double damping, double stiffness)
    {
        var ease = new SpringEase { DampingRatio = damping, Stiffness = stiffness };
        Assert.Equal(0, ease.Ease(0), 6);
        Assert.Equal(1, ease.Ease(1), 6);
    }

    [Fact]
    public void LowDampingOvershootsThenSettles()
    {
        var ease = new SpringEase { DampingRatio = 0.66, Stiffness = 11 };
        double peak = Enumerable.Range(0, 101).Select(i => ease.Ease(i / 100.0)).Max();

        Assert.True(peak > 1.0, $"期望有回弹，最大值 {peak}");
        Assert.True(peak < 1.15, $"回弹过大：{peak}");
        Assert.InRange(ease.Ease(0.9), 0.98, 1.02);
    }

    [Fact]
    public void HighDampingBarelyOvershoots()
    {
        var ease = new SpringEase { DampingRatio = 0.92, Stiffness = 12 };
        double peak = Enumerable.Range(0, 101).Select(i => ease.Ease(i / 100.0)).Max();
        Assert.True(peak < 1.01, $"收起动画不应明显回弹：{peak}");
    }
}
