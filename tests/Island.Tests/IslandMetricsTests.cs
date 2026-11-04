using Island.Core;

namespace Island.Tests;

public class IslandMetricsTests
{
    public static TheoryData<IslandState, bool> States()
    {
        var data = new TheoryData<IslandState, bool>();
        foreach (IslandState state in Enum.GetValues<IslandState>())
        {
            data.Add(state, true);
            data.Add(state, false);
        }
        return data;
    }

    [Theory]
    [MemberData(nameof(States))]
    public void EveryStateFitsInsideTheHostAndWindow(IslandState state, bool idleAsLine)
    {
        IslandMetrics m = IslandMetrics.For(state, idleAsLine);
        Assert.True(m.Width <= IslandMetrics.HostWidth, $"{state} 太宽");
        Assert.True(m.Bottom <= IslandMetrics.HostHeight, $"{state} 超出胶囊区域");
        Assert.True(IslandMetrics.HostWidth + 40 <= IslandMetrics.WindowWidth, "窗口要给阴影留空间");
        Assert.True(IslandMetrics.HostHeight + 20 <= IslandMetrics.WindowHeight, "窗口要给阴影留空间");
    }

    [Theory]
    [MemberData(nameof(States))]
    public void CornerRadiusNeverExceedsHalfTheHeight(IslandState state, bool idleAsLine)
    {
        IslandMetrics m = IslandMetrics.For(state, idleAsLine);
        Assert.True(m.CornerRadius <= m.Height / 2);
    }

    [Fact]
    public void IdleLineHugsTheTopEdgeAndShowsOnlyAThinStrip()
    {
        IslandMetrics line = IslandMetrics.For(IslandState.Idle, idleAsLine: true);
        Assert.True(line.Top < 0, "细线的上半截应在屏幕外");
        Assert.Equal(IslandMetrics.LineVisibleHeight, line.Bottom);
        Assert.True(line.Bottom <= 6, "细线不应挡住浏览器标签");
    }
}
