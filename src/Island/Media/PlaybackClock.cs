using System.IO;
using System.Text.Json;

namespace Island.Media;

/// <summary>
/// 有些播放器（比如网易云音乐）不向系统报告播放进度，这时岛自己计时：
/// 换歌时从 0 开始，播放时累加，暂停时停住。播放器里拖动进度条后会有偏差，换下一首就恢复。
/// </summary>
public sealed class PlaybackClock
{
    private string? _track;
    private TimeSpan _accumulated;
    private DateTimeOffset? _playingSince;

    /// <summary>每次媒体信息变化时调用。</summary>
    public void Update(string track, bool playing, DateTimeOffset now)
    {
        if (track != _track)
        {
            _track = track;
            _accumulated = TimeSpan.Zero;
            _playingSince = playing ? now : null;
            return;
        }

        if (playing && _playingSince is null)
        {
            _playingSince = now;
        }
        else if (!playing && _playingSince is { } since)
        {
            _accumulated += Clamp(now - since);
            _playingSince = null;
        }
    }

    public TimeSpan Elapsed(DateTimeOffset now) =>
        _playingSince is { } since ? _accumulated + Clamp(now - since) : _accumulated;

    public void Reset()
    {
        _track = null;
        _accumulated = TimeSpan.Zero;
        _playingSince = null;
    }

    private static TimeSpan Clamp(TimeSpan t) => t < TimeSpan.Zero ? TimeSpan.Zero : t;
}

/// <summary>
/// 从网易云音乐的本地播放记录里查歌曲时长（它不向系统报告进度和时长）。
/// 记录文件：%LOCALAPPDATA%\NetEase\CloudMusic\webdata\file\history，
/// 内容是最近播放的列表，每项的 track 里有 name、artists、duration（毫秒）。
/// 格式不是公开接口，读不到或对不上就当不知道时长。
/// </summary>
public static class NeteaseHistory
{
    public static string DefaultPath => Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
        "NetEase", "CloudMusic", "webdata", "file", "history");

    public static TimeSpan? TryGetDuration(string title, string artist, string? path = null)
    {
        try
        {
            path ??= DefaultPath;
            if (!File.Exists(path)) return null;
            // 网易云正在写的时候也能读
            using var stream = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete);
            using var reader = new StreamReader(stream);
            return FindDuration(reader.ReadToEnd(), title, artist);
        }
        catch
        {
            return null;
        }
    }

    /// <summary>在记录 JSON 里找歌名一致（有歌手时歌手也要对上）的第一项，返回它的时长。</summary>
    internal static TimeSpan? FindDuration(string json, string title, string artist)
    {
        using var doc = JsonDocument.Parse(json);
        if (doc.RootElement.ValueKind != JsonValueKind.Array) return null;

        foreach (var item in doc.RootElement.EnumerateArray())
        {
            if (item.ValueKind != JsonValueKind.Object) continue;
            var track = item.TryGetProperty("track", out var t) ? t : item;
            if (track.ValueKind != JsonValueKind.Object) continue;
            if (!track.TryGetProperty("name", out var name) || name.ValueKind != JsonValueKind.String) continue;
            if (!string.Equals(name.GetString()?.Trim(), title.Trim(), StringComparison.OrdinalIgnoreCase)) continue;
            if (!ArtistMatches(track, artist)) continue;

            if (track.TryGetProperty("duration", out var d) && d.TryGetInt64(out long ms) && ms > 0)
                return TimeSpan.FromMilliseconds(ms);
            if (track.TryGetProperty("dt", out var dt) && dt.TryGetInt64(out ms) && ms > 0)
                return TimeSpan.FromMilliseconds(ms);
        }
        return null;
    }

    private static bool ArtistMatches(JsonElement track, string artist)
    {
        if (string.IsNullOrWhiteSpace(artist)) return true;
        foreach (string key in new[] { "artists", "ar" })
        {
            if (!track.TryGetProperty(key, out var artists) || artists.ValueKind != JsonValueKind.Array) continue;
            foreach (var a in artists.EnumerateArray())
            {
                if (a.TryGetProperty("name", out var n) && n.ValueKind == JsonValueKind.String &&
                    !string.IsNullOrWhiteSpace(n.GetString()) &&
                    artist.Contains(n.GetString()!, StringComparison.OrdinalIgnoreCase))
                    return true;
            }
            return false;
        }
        return true; // 记录里没有歌手信息，只按歌名匹配
    }
}
