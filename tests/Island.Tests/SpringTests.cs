using Island.Animation;

namespace Island.Tests;

public class SpringTests
{
    private static int StepsUntilSettled(Spring spring, SpringParameters p, double frame = 1.0 / 60, int limit = 600)
    {
        for (int i = 1; i <= limit; i++)
        {
            spring.Step(frame, p);
            if (spring.IsSettled(0.05)) return i;
        }
        return int.MaxValue;
    }

    [Fact]
    public void ExpandSpringSettlesQuicklyWithASmallOvershoot()
    {
        var spring = new Spring(34) { Target = 260 };
        double peak = 0;
        for (int i = 0; i < 120; i++)
        {
            spring.Step(1.0 / 60, SpringParameters.Expand);
            peak = Math.Max(peak, spring.Value);
        }

        Assert.True(peak > 260, "展开应有一点回弹");
        Assert.True(peak < 260 + 0.12 * (260 - 34), $"回弹过大：{peak}");
        Assert.True(StepsUntilSettled(new Spring(34) { Target = 260 }, SpringParameters.Expand) < 60, "应在 1 秒内停下");
    }

    [Fact]
    public void CollapseSpringIsFastAndBarelyOvershoots()
    {
        var spring = new Spring(260) { Target = 10 };
        double low = double.MaxValue;
        for (int i = 0; i < 120; i++)
        {
            spring.Step(1.0 / 60, SpringParameters.Collapse);
            low = Math.Min(low, spring.Value);
        }
        Assert.True(low > 10 - 2, $"收起不应明显回弹：{low}");
        Assert.True(StepsUntilSettled(new Spring(260) { Target = 10 }, SpringParameters.Collapse) < 40);
    }

    [Fact]
    public void RetargetingMidFlightKeepsVelocity()
    {
        var spring = new Spring(0) { Target = 100 };
        for (int i = 0; i < 6; i++) spring.Step(1.0 / 60, SpringParameters.Expand);
        double velocityBefore = spring.Velocity;
        double valueBefore = spring.Value;

        spring.Target = 0; // 半路改主意
        spring.Step(1.0 / 240, SpringParameters.Collapse);

        Assert.True(velocityBefore > 0);
        Assert.True(spring.Value > valueBefore, "换目标后的下一瞬间仍在沿原方向运动，不会突然反向跳动");
    }

    [Fact]
    public void MotionIsIndependentOfFrameRate()
    {
        var at60 = new Spring(0) { Target = 100 };
        var at144 = new Spring(0) { Target = 100 };
        for (int i = 0; i < 18; i++) at60.Step(1.0 / 60, SpringParameters.Expand);    // 0.3 秒
        for (int i = 0; i < 43; i++) at144.Step(1.0 / 144, SpringParameters.Expand);  // ≈0.2986 秒
        at144.Step(0.3 - 43.0 / 144, SpringParameters.Expand);
        Assert.InRange(Math.Abs(at60.Value - at144.Value), 0, 0.2);
    }
}
