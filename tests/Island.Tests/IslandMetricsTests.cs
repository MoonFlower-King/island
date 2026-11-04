using Island.Core;

namespace Island.Tests;

public class IslandMetricsTests
{
    public static TheoryData<IslandState> States => new(Enum.GetValues<IslandState>());

    [Theory]
    [MemberData(nameof(States))]
    public void EveryStateFitsInsideTheWindowWithRoomForShadow(IslandState state)
    {
        IslandMetrics m = IslandMetrics.For(state);
        Assert.True(m.Width + 20 <= IslandMetrics.WindowWidth, $"{state} 太宽");
        Assert.True(m.Height + IslandMetrics.TopMargin + 20 <= IslandMetrics.WindowHeight, $"{state} 太高");
    }

    [Theory]
    [MemberData(nameof(States))]
    public void CornerRadiusNeverExceedsHalfTheHeight(IslandState state)
    {
        IslandMetrics m = IslandMetrics.For(state);
        Assert.True(m.CornerRadius <= m.Height / 2);
    }
}
