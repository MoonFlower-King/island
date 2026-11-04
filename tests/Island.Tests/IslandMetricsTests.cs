using Island.Core;

namespace Island.Tests;

public class IslandMetricsTests
{
    public static TheoryData<IslandState, bool, DockEdge> States()
    {
        var data = new TheoryData<IslandState, bool, DockEdge>();
        foreach (IslandState state in Enum.GetValues<IslandState>())
        foreach (DockEdge edge in Enum.GetValues<DockEdge>())
        {
            data.Add(state, true, edge);
            data.Add(state, false, edge);
        }
        return data;
    }

    [Theory]
    [MemberData(nameof(States))]
    public void EveryStateFitsInTheWindowWithRoomForTheShadow(IslandState state, bool idleAsLine, DockEdge edge)
    {
        IslandMetrics m = IslandMetrics.For(state, idleAsLine, edge);
        IslandRect r = m.RectIn(edge);
        const double shadow = 18;

        switch (edge)
        {
            case DockEdge.Top:
                Assert.True(r.X >= shadow && r.Right <= IslandMetrics.WindowWidth - shadow, $"{state} 太宽");
                Assert.True(r.Bottom <= IslandMetrics.WindowHeight - shadow, $"{state} 太高");
                break;
            case DockEdge.Left:
                Assert.True(r.Right <= IslandMetrics.WindowWidth - shadow, $"{state} 太宽");
                Assert.True(r.Y >= 0 && r.Bottom <= IslandMetrics.WindowHeight, $"{state} 太高");
                break;
            case DockEdge.Right:
                Assert.True(r.X >= shadow, $"{state} 太宽");
                Assert.True(r.Y >= 0 && r.Bottom <= IslandMetrics.WindowHeight, $"{state} 太高");
                break;
        }
    }

    [Theory]
    [MemberData(nameof(States))]
    public void CornerRadiusNeverExceedsHalfTheShortSide(IslandState state, bool idleAsLine, DockEdge edge)
    {
        IslandMetrics m = IslandMetrics.For(state, idleAsLine, edge);
        Assert.True(m.CornerRadius <= Math.Min(m.Width, m.Height) / 2);
    }

    [Fact]
    public void IdleLineHugsTheTopEdgeAndShowsOnlyAThinStrip()
    {
        IslandMetrics line = IslandMetrics.For(IslandState.Idle, idleAsLine: true);
        Assert.True(line.Top < 0, "细线的上半截应在屏幕外");
        Assert.Equal(IslandMetrics.LineVisibleHeight, line.Bottom);
        Assert.True(line.Bottom <= 6, "细线不应挡住浏览器标签");
        IslandRect r = line.RectIn(DockEdge.Top);
        Assert.Equal(IslandMetrics.WindowWidth / 2, r.X + r.Width / 2);
    }

    [Theory]
    [InlineData(DockEdge.Left)]
    [InlineData(DockEdge.Right)]
    public void SideLineIsVerticalAndHugsItsEdge(DockEdge edge)
    {
        IslandMetrics line = IslandMetrics.For(IslandState.Idle, idleAsLine: true, edge);
        Assert.True(line.Height > line.Width * 5, "两侧的细线应该是竖的");
        IslandRect r = line.RectIn(edge);
        Assert.Equal(IslandMetrics.WindowHeight / 2, r.Y + r.Height / 2);
        if (edge == DockEdge.Left)
        {
            Assert.True(r.X < 0 && r.Right == IslandMetrics.LineVisibleHeight, "一半藏在屏幕左边外");
        }
        else
        {
            Assert.True(r.Right > IslandMetrics.WindowWidth, "一半藏在屏幕右边外");
            Assert.Equal(IslandMetrics.WindowWidth - IslandMetrics.LineVisibleHeight, r.X);
        }
    }

    [Theory]
    [InlineData(DockEdge.Left)]
    [InlineData(DockEdge.Right)]
    public void PanelsOpenFromTheSideEdge(DockEdge edge)
    {
        IslandRect r = IslandMetrics.For(IslandState.Expanded, edge: edge).RectIn(edge);
        double gap = edge == DockEdge.Left ? r.X : IslandMetrics.WindowWidth - r.Right;
        Assert.Equal(IslandMetrics.TopMargin, gap);
    }
}
