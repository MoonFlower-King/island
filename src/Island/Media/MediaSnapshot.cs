using System.Windows.Media;

namespace Island.Media;

/// <summary>
/// 某一时刻正在播放的媒体信息（不可变，可以跨线程传递）。
/// </summary>
public sealed record MediaSnapshot(
    string Title,
    string Artist,
    string SourceApp,
    ImageSource? AlbumArt,
    Color Accent,
    bool IsPlaying,
    TimeSpan Position,
    TimeSpan Duration,
    DateTimeOffset PositionUpdatedAt,
    double PlaybackRate,
    bool CanGoPrevious,
    bool CanGoNext,
    bool CanPlayPause)
{
    /// <summary>有些应用（例如部分浏览器）不报告进度，这时隐藏进度条。</summary>
    public bool HasTimeline => Duration > TimeSpan.Zero;

    /// <summary>
    /// 系统只在状态变化时更新进度，这里根据上次更新时间推算当前进度。
    /// </summary>
    public TimeSpan EstimatePosition(DateTimeOffset now)
    {
        if (!HasTimeline) return TimeSpan.Zero;

        TimeSpan position = Position;
        if (IsPlaying && PositionUpdatedAt != default)
        {
            TimeSpan elapsed = now - PositionUpdatedAt;
            if (elapsed > TimeSpan.Zero)
                position += TimeSpan.FromTicks((long)(elapsed.Ticks * PlaybackRate));
        }

        if (position < TimeSpan.Zero) return TimeSpan.Zero;
        return position > Duration ? Duration : position;
    }
}
