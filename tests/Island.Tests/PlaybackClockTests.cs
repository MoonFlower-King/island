using System.IO;
using Island.Media;

namespace Island.Tests;

public class PlaybackClockTests
{
    private static readonly DateTimeOffset T0 = new(2026, 11, 4, 12, 0, 0, TimeSpan.Zero);

    [Fact]
    public void CountsOnlyWhilePlaying()
    {
        var clock = new PlaybackClock();
        clock.Update("a", playing: true, T0);
        Assert.Equal(TimeSpan.FromSeconds(10), clock.Elapsed(T0.AddSeconds(10)));

        clock.Update("a", playing: false, T0.AddSeconds(10));
        Assert.Equal(TimeSpan.FromSeconds(10), clock.Elapsed(T0.AddSeconds(60)));

        clock.Update("a", playing: true, T0.AddSeconds(60));
        Assert.Equal(TimeSpan.FromSeconds(15), clock.Elapsed(T0.AddSeconds(65)));
    }

    [Fact]
    public void RestartsOnNewTrackAndIgnoresDuplicateUpdates()
    {
        var clock = new PlaybackClock();
        clock.Update("a", true, T0);
        clock.Update("a", true, T0.AddSeconds(5)); // 重复通知不应该重新计时
        Assert.Equal(TimeSpan.FromSeconds(8), clock.Elapsed(T0.AddSeconds(8)));

        clock.Update("b", true, T0.AddSeconds(8));
        Assert.Equal(TimeSpan.FromSeconds(2), clock.Elapsed(T0.AddSeconds(10)));

        clock.Update("c", false, T0.AddSeconds(10));
        Assert.Equal(TimeSpan.Zero, clock.Elapsed(T0.AddSeconds(30)));
    }

    private const string History = """
        [
          {"track": {"name": "光年之外", "duration": 235000, "artists": [{"name": "G.E.M.邓紫棋"}]}, "playtime": 1},
          {"track": {"name": "晴天", "dt": 269000, "ar": [{"name": "周杰伦"}]}},
          {"track": {"name": "同名歌", "duration": 100000, "artists": [{"name": "甲"}]}},
          {"track": {"name": "同名歌", "duration": 200000, "artists": [{"name": "乙"}]}}
        ]
        """;

    [Fact]
    public void FindsDurationInNeteaseHistory()
    {
        Assert.Equal(TimeSpan.FromSeconds(235), NeteaseHistory.FindDuration(History, "光年之外", "G.E.M.邓紫棋"));
        Assert.Equal(TimeSpan.FromSeconds(269), NeteaseHistory.FindDuration(History, "晴天", "周杰伦"));
        Assert.Equal(TimeSpan.FromSeconds(200), NeteaseHistory.FindDuration(History, "同名歌", "乙"));
        Assert.Equal(TimeSpan.FromSeconds(235), NeteaseHistory.FindDuration(History, "光年之外", ""));
        Assert.Null(NeteaseHistory.FindDuration(History, "不存在", "谁"));
        Assert.Null(NeteaseHistory.FindDuration("{}", "晴天", ""));
    }

    [Fact]
    public void MissingHistoryFileMeansUnknown() =>
        Assert.Null(NeteaseHistory.TryGetDuration("晴天", "周杰伦", Path.Combine(Path.GetTempPath(), Guid.NewGuid().ToString("N"))));
}
