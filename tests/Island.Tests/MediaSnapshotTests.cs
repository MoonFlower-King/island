using System.Windows.Media;
using Island.Media;

namespace Island.Tests;

public class MediaSnapshotTests
{
    private static readonly DateTimeOffset T0 = new(2026, 1, 1, 12, 0, 0, TimeSpan.Zero);

    private static MediaSnapshot Make(bool playing, TimeSpan position, TimeSpan duration, double rate = 1) =>
        new("歌", "歌手", "App", null, Colors.White, playing, position, duration, T0, rate, true, true, true);

    [Fact]
    public void PausedPositionDoesNotAdvance()
    {
        var m = Make(playing: false, TimeSpan.FromSeconds(30), TimeSpan.FromMinutes(3));
        Assert.Equal(TimeSpan.FromSeconds(30), m.EstimatePosition(T0.AddSeconds(10)));
    }

    [Fact]
    public void PlayingPositionAdvancesWithRate()
    {
        var m = Make(playing: true, TimeSpan.FromSeconds(30), TimeSpan.FromMinutes(3), rate: 1.5);
        Assert.Equal(TimeSpan.FromSeconds(45), m.EstimatePosition(T0.AddSeconds(10)));
    }

    [Fact]
    public void PositionIsClampedToDuration()
    {
        var m = Make(playing: true, TimeSpan.FromSeconds(170), TimeSpan.FromSeconds(180));
        Assert.Equal(TimeSpan.FromSeconds(180), m.EstimatePosition(T0.AddMinutes(5)));
    }

    [Fact]
    public void WithoutTimelineReportsZero()
    {
        var m = Make(playing: true, TimeSpan.FromSeconds(10), TimeSpan.Zero);
        Assert.False(m.HasTimeline);
        Assert.Equal(TimeSpan.Zero, m.EstimatePosition(T0.AddSeconds(10)));
    }
}
