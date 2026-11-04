using System.Diagnostics;
using System.IO;
using System.Windows.Media;
using System.Windows.Media.Imaging;
using System.Windows.Threading;
using Windows.Media.Control;

namespace Island.Media;

/// <summary>
/// 通过 Windows 的系统媒体传输控件（SMTC）获取“正在播放”的信息并控制播放。
/// Spotify、网易云音乐、QQ 音乐、系统媒体播放器、浏览器里的视频等，只要接入了系统媒体浮层，都能读到。
///
/// SMTC 的事件在后台线程触发；本类在后台整理好数据，再回到 UI 线程触发 <see cref="Changed"/>。
/// </summary>
public sealed class MediaSessionService : IDisposable
{
    private readonly Dispatcher _dispatcher;
    private readonly object _gate = new();
    private readonly SemaphoreSlim _readLock = new(1, 1); // 串行读取，保护下面的封面缓存

    private GlobalSystemMediaTransportControlsSessionManager? _manager;
    private GlobalSystemMediaTransportControlsSession? _session;
    private int _refreshVersion;
    private bool _disposed;

    // 封面只在换歌时重新解码
    private string? _artKey;
    private ImageSource? _art;
    private Color _accent = DefaultAccent;

    private static readonly Color DefaultAccent = Color.FromRgb(0x30, 0xD1, 0x58);

    public MediaSessionService(Dispatcher dispatcher) => _dispatcher = dispatcher;

    /// <summary>当前媒体信息；没有可用的媒体会话时为 null。只在 UI 线程读写。</summary>
    public MediaSnapshot? Current { get; private set; }

    /// <summary>媒体信息变化（在 UI 线程触发）。</summary>
    public event Action<MediaSnapshot?>? Changed;

    public async Task StartAsync()
    {
        try
        {
            _manager = await GlobalSystemMediaTransportControlsSessionManager.RequestAsync();
        }
        catch (Exception ex)
        {
            // 系统太旧或 SMTC 服务不可用时，岛照常运行，只是没有音乐功能
            Debug.WriteLine($"[Media] SMTC 不可用：{ex}");
            return;
        }

        if (_disposed) return;

        _manager.CurrentSessionChanged += OnCurrentSessionChanged;
        _manager.SessionsChanged += OnSessionsChanged;
        AttachCurrentSession();
    }

    // ───────────────────────────── 播放控制 ─────────────────────────────

    public Task TogglePlayPauseAsync() => RunControlAsync(s => s.TryTogglePlayPauseAsync().AsTask());

    public Task NextAsync() => RunControlAsync(s => s.TrySkipNextAsync().AsTask());

    public Task PreviousAsync() => RunControlAsync(s => s.TrySkipPreviousAsync().AsTask());

    private async Task RunControlAsync(Func<GlobalSystemMediaTransportControlsSession, Task<bool>> action)
    {
        GlobalSystemMediaTransportControlsSession? session;
        lock (_gate) session = _session;
        if (session is null) return;

        try
        {
            await action(session);
        }
        catch (Exception ex)
        {
            Debug.WriteLine($"[Media] 控制失败：{ex.Message}");
        }
    }

    // ───────────────────────────── 会话跟踪 ─────────────────────────────

    private void OnCurrentSessionChanged(GlobalSystemMediaTransportControlsSessionManager sender,
        CurrentSessionChangedEventArgs args) => AttachCurrentSession();

    private void OnSessionsChanged(GlobalSystemMediaTransportControlsSessionManager sender,
        SessionsChangedEventArgs args) => AttachCurrentSession();

    private void AttachCurrentSession()
    {
        lock (_gate)
        {
            if (_disposed || _manager is null) return;

            GlobalSystemMediaTransportControlsSession? next;
            try
            {
                next = _manager.GetCurrentSession();
            }
            catch (Exception ex)
            {
                Debug.WriteLine($"[Media] 获取当前会话失败：{ex.Message}");
                next = null;
            }

            if (!ReferenceEquals(next, _session))
            {
                DetachSession();
                _session = next;
                if (_session is not null)
                {
                    _session.MediaPropertiesChanged += OnMediaPropertiesChanged;
                    _session.PlaybackInfoChanged += OnPlaybackInfoChanged;
                    _session.TimelinePropertiesChanged += OnTimelinePropertiesChanged;
                }
            }
        }

        _ = RefreshAsync();
    }

    private void DetachSession()
    {
        if (_session is null) return;
        _session.MediaPropertiesChanged -= OnMediaPropertiesChanged;
        _session.PlaybackInfoChanged -= OnPlaybackInfoChanged;
        _session.TimelinePropertiesChanged -= OnTimelinePropertiesChanged;
        _session = null;
    }

    private void OnMediaPropertiesChanged(GlobalSystemMediaTransportControlsSession sender,
        MediaPropertiesChangedEventArgs args) => _ = RefreshAsync();

    private void OnPlaybackInfoChanged(GlobalSystemMediaTransportControlsSession sender,
        PlaybackInfoChangedEventArgs args) => _ = RefreshAsync();

    private void OnTimelinePropertiesChanged(GlobalSystemMediaTransportControlsSession sender,
        TimelinePropertiesChangedEventArgs args) => _ = RefreshAsync();

    // ───────────────────────────── 读取数据 ─────────────────────────────

    private async Task RefreshAsync()
    {
        // 事件经常成串到达，只采用最后一次刷新的结果
        int version = Interlocked.Increment(ref _refreshVersion);

        GlobalSystemMediaTransportControlsSession? session;
        lock (_gate) session = _session;

        MediaSnapshot? snapshot = null;
        if (session is not null)
        {
            await _readLock.WaitAsync();
            try
            {
                snapshot = await ReadSnapshotAsync(session);
            }
            catch (Exception ex)
            {
                // 应用关闭的瞬间读取会话可能抛异常，忽略即可
                Debug.WriteLine($"[Media] 读取媒体信息失败：{ex.Message}");
            }
            finally
            {
                _readLock.Release();
            }
        }

        if (version != Volatile.Read(ref _refreshVersion) || _disposed) return;

        await _dispatcher.InvokeAsync(() =>
        {
            if (version != Volatile.Read(ref _refreshVersion) || _disposed) return;
            Current = snapshot;
            Changed?.Invoke(snapshot);
        });
    }

    private async Task<MediaSnapshot?> ReadSnapshotAsync(GlobalSystemMediaTransportControlsSession session)
    {
        var props = await session.TryGetMediaPropertiesAsync();
        if (props is null || string.IsNullOrWhiteSpace(props.Title)) return null;

        var playback = session.GetPlaybackInfo();
        var timeline = session.GetTimelineProperties();

        string artist = !string.IsNullOrWhiteSpace(props.Artist) ? props.Artist : props.AlbumArtist ?? "";
        string artKey = $"{session.SourceAppUserModelId}|{props.Title}|{artist}|{props.AlbumTitle}";

        if (artKey != _artKey)
        {
            (_art, _accent) = await LoadArtAsync(props.Thumbnail);
            _artKey = artKey;
        }

        var controls = playback?.Controls;
        TimeSpan duration = timeline.EndTime - timeline.StartTime;

        return new MediaSnapshot(
            Title: props.Title.Trim(),
            Artist: artist.Trim(),
            SourceApp: FriendlyAppName(session.SourceAppUserModelId),
            AlbumArt: _art,
            Accent: _accent,
            IsPlaying: playback?.PlaybackStatus == GlobalSystemMediaTransportControlsSessionPlaybackStatus.Playing,
            Position: timeline.Position - timeline.StartTime,
            Duration: duration > TimeSpan.Zero ? duration : TimeSpan.Zero,
            PositionUpdatedAt: timeline.LastUpdatedTime,
            PlaybackRate: playback?.PlaybackRate ?? 1.0,
            CanGoPrevious: controls?.IsPreviousEnabled ?? false,
            CanGoNext: controls?.IsNextEnabled ?? false,
            CanPlayPause: controls?.IsPlayPauseToggleEnabled ?? false);
    }

    private static async Task<(ImageSource?, Color)> LoadArtAsync(
        Windows.Storage.Streams.IRandomAccessStreamReference? thumbnail)
    {
        if (thumbnail is null) return (null, DefaultAccent);

        try
        {
            using var winrtStream = await thumbnail.OpenReadAsync();
            using var source = winrtStream.AsStream();
            var buffer = new MemoryStream();
            await source.CopyToAsync(buffer);
            buffer.Position = 0;

            var image = new BitmapImage();
            image.BeginInit();
            image.CacheOption = BitmapCacheOption.OnLoad; // 立即解码，之后可以释放流
            image.DecodePixelWidth = 160;
            image.StreamSource = buffer;
            image.EndInit();
            image.Freeze(); // 冻结后才能交给 UI 线程使用

            return (image, ColorExtractor.Accent(image, DefaultAccent));
        }
        catch (Exception ex)
        {
            Debug.WriteLine($"[Media] 封面加载失败：{ex.Message}");
            return (null, DefaultAccent);
        }
    }

    /// <summary>
    /// SourceAppUserModelId 形如 "Spotify.exe" 或 "Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic"，
    /// 转成稍微好读一点的名字。
    /// </summary>
    private static string FriendlyAppName(string? aumid)
    {
        if (string.IsNullOrWhiteSpace(aumid)) return "";

        string name = aumid;
        int bang = name.LastIndexOf('!');
        if (bang >= 0) name = name[(bang + 1)..];
        if (name.EndsWith(".exe", StringComparison.OrdinalIgnoreCase)) name = name[..^4];
        int dot = name.LastIndexOf('.');
        if (dot >= 0 && dot < name.Length - 1) name = name[(dot + 1)..];

        return name switch
        {
            "cloudmusic" => "网易云音乐",
            "QQMusic" => "QQ 音乐",
            "ZuneMusic" => "媒体播放器",
            "chrome" => "Chrome",
            "msedge" => "Edge",
            "firefox" => "Firefox",
            _ => name,
        };
    }

    public void Dispose()
    {
        lock (_gate)
        {
            if (_disposed) return;
            _disposed = true;

            DetachSession();
            if (_manager is not null)
            {
                _manager.CurrentSessionChanged -= OnCurrentSessionChanged;
                _manager.SessionsChanged -= OnSessionsChanged;
                _manager = null;
            }
        }
    }
}
