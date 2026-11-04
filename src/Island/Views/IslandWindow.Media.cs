using System.Windows;
using System.Windows.Media;
using System.Windows.Media.Animation;
using System.Windows.Shapes;
using System.Windows.Threading;
using Island.Core;
using Island.Media;

namespace Island.Views;

/// <summary>
/// 岛上的“正在播放”：
///   · 播放中 → 紧凑胶囊：封面 + 歌名 + 律动条
///   · 悬停   → 展开面板的“音乐”标签：封面、歌曲信息、进度条、上一首/播放暂停/下一首
///   · 暂停后 → 继续显示一会儿再缩回待机，方便马上恢复播放
/// </summary>
public partial class IslandWindow
{
    private const string GlyphPlay = "";
    private const string GlyphPause = "";

    /// <summary>暂停后紧凑胶囊继续停留的时间。</summary>
    private static readonly TimeSpan PausedLinger = TimeSpan.FromSeconds(20);

    private readonly DispatcherTimer _mediaLingerTimer = new() { Interval = PausedLinger };
    private readonly DispatcherTimer _progressTimer = new() { Interval = TimeSpan.FromMilliseconds(500) };

    /// <summary>播放器不报告进度时（网易云音乐），岛自己计时。</summary>
    private readonly PlaybackClock _clock = new();
    private string? _fallbackTrack;
    private TimeSpan? _fallbackDuration;

    private MediaSessionService? _mediaService;
    private MediaSnapshot? _media;
    private bool _mediaLingering;
    private bool _equalizerRunning;
    private Color _rawAccent = Color.FromRgb(0x30, 0xD1, 0x58);

    /// <summary>音乐是否要占用紧凑胶囊：正在播放，或刚暂停不久。</summary>
    private bool MediaWantsCompact => _media is { } m && (m.IsPlaying || _mediaLingering);

    private Rectangle[] EqualizerBars => [EqBar1, EqBar2, EqBar3, EqBar4];

    private void InitializeMedia()
    {
        foreach (var bar in EqualizerBars)
            bar.RenderTransform = new ScaleTransform(1, 0.3);
        ApplyAccent(Color.FromRgb(0x30, 0xD1, 0x58));

        _mediaLingerTimer.Tick += (_, _) =>
        {
            _mediaLingerTimer.Stop();
            _mediaLingering = false;
            UpdateState();
        };
        _progressTimer.Tick += (_, _) => UpdateProgress();

        _mediaService = new MediaSessionService(Dispatcher);
        _mediaService.Changed += OnMediaChanged;
        Loaded += async (_, _) =>
        {
            if (_mediaService is not null) await _mediaService.StartAsync();
        };
    }

    private void ShutdownMedia()
    {
        _mediaLingerTimer.Stop();
        _progressTimer.Stop();
        if (_mediaService is not null)
        {
            _mediaService.Changed -= OnMediaChanged;
            _mediaService.Dispose();
            _mediaService = null;
        }
    }

    private void OnMediaChanged(MediaSnapshot? snapshot)
    {
        bool wasPlaying = _media?.IsPlaying == true;
        _media = snapshot;

        if (snapshot is null)
        {
            _clock.Reset();
            _fallbackTrack = null;
            _mediaLingering = false;
            _mediaLingerTimer.Stop();
        }
        else if (snapshot.IsPlaying)
        {
            _mediaLingering = false;
            _mediaLingerTimer.Stop();
        }
        else if (wasPlaying)
        {
            // 刚刚暂停：先别缩回去
            _mediaLingering = true;
            _mediaLingerTimer.Stop();
            _mediaLingerTimer.Start();
        }

        if (snapshot is not null) ApplyMediaToUi(snapshot);
        UpdateLineTint();
        MediaPlayer.Visibility = snapshot is null ? Visibility.Collapsed : Visibility.Visible;
        MusicEmpty.Visibility = snapshot is null ? Visibility.Visible : Visibility.Collapsed;

        UpdateState();
        RefreshMediaAnimations();
    }

    private void ApplyMediaToUi(MediaSnapshot m)
    {
        MediaCompactTitle.Text = string.IsNullOrEmpty(m.Artist) ? m.Title : $"{m.Title} · {m.Artist}";
        MediaTitle.Text = m.Title;
        MediaArtist.Text = string.IsNullOrEmpty(m.Artist) ? m.SourceApp : m.Artist;
        MediaSource.Text = string.IsNullOrEmpty(m.Artist) ? "" : m.SourceApp;

        SetArt(MediaCompactArt, MediaCompactArtPlaceholder, m.AlbumArt);
        SetArt(MediaExpandedArt, MediaExpandedArtPlaceholder, m.AlbumArt);
        ApplyAccent(m.Accent);

        PlayPauseButton.Content = m.IsPlaying ? GlyphPause : GlyphPlay;
        PlayPauseButton.IsEnabled = m.CanPlayPause;
        PreviousButton.IsEnabled = m.CanGoPrevious;
        NextButton.IsEnabled = m.CanGoNext;

        UpdateFallbackTimeline(m);
        MediaTimeline.Visibility = Visibility.Visible;
        UpdateProgress();
    }

    /// <summary>
    /// 播放器没报告进度时：自己从换歌那一刻开始计时；网易云音乐的时长从它的本地播放记录里查。
    /// </summary>
    private void UpdateFallbackTimeline(MediaSnapshot m)
    {
        if (m.HasTimeline)
        {
            _clock.Reset();
            _fallbackTrack = null;
            _fallbackDuration = null;
            MediaTimeline.ToolTip = null;
            return;
        }

        string track = $"{m.SourceApp}|{m.Title}|{m.Artist}";
        _clock.Update(track, m.IsPlaying, DateTimeOffset.Now);
        MediaTimeline.ToolTip = "这个播放器不向系统报告进度，这里是岛按播放时间估算的；在播放器里拖动进度后会有偏差";
        if (track == _fallbackTrack) return;

        _fallbackTrack = track;
        _fallbackDuration = null;
        if (m.SourceApp != "网易云音乐") return;

        // 读文件放到后台，读完如果还是这首歌再用
        string title = m.Title, artist = m.Artist;
        _ = Task.Run(() => NeteaseHistory.TryGetDuration(title, artist)).ContinueWith(t =>
        {
            if (_fallbackTrack != track || t.Result is not { } duration) return;
            _fallbackDuration = duration;
            UpdateProgress();
        }, TaskScheduler.FromCurrentSynchronizationContext());
    }

    private static void SetArt(System.Windows.Controls.Border host, UIElement placeholder, ImageSource? art)
    {
        if (art is null)
        {
            host.SetResourceReference(System.Windows.Controls.Border.BackgroundProperty, "IslandSurfaceStrong");
            placeholder.Visibility = Visibility.Visible;
            return;
        }

        var brush = new ImageBrush(art) { Stretch = Stretch.UniformToFill };
        brush.Freeze();
        host.Background = brush;
        placeholder.Visibility = Visibility.Collapsed;
    }

    /// <summary>封面取色按当前主题的背景调整：深色背景提亮，浅色背景压暗，保证看得清。</summary>
    private void ApplyAccent(Color color)
    {
        _rawAccent = color;
        Color readable = ColorMath.EnsureReadable(color, App.Theme.Current.Background);
        var brush = new SolidColorBrush(readable);
        brush.Freeze();
        ProgressFill.Background = brush;
        foreach (var bar in EqualizerBars) bar.Fill = brush;
        _accentBrush = brush;
        UpdateLineTint();
    }

    private Brush? _accentBrush;

    /// <summary>“播放音乐时也缩成细线”时，播放中的细线变成封面的主题色，一眼就知道在放歌。</summary>
    private void UpdateLineTint() =>
        LineView.Background = _settings.MediaAsLine && _media?.IsPlaying == true ? _accentBrush : null;

    // ───────────────────────────── 进度 ─────────────────────────────

    private void UpdateProgress()
    {
        if (_media is not { } m) return;

        TimeSpan position;
        TimeSpan? duration;
        if (m.HasTimeline)
        {
            position = m.EstimatePosition(DateTimeOffset.Now);
            duration = m.Duration;
        }
        else
        {
            position = _clock.Elapsed(DateTimeOffset.Now);
            duration = _fallbackDuration;
            if (duration is { } d && position > d) position = d;
        }

        if (duration is { Ticks: > 0 } total)
        {
            double ratio = (double)position.Ticks / total.Ticks;
            ProgressFill.Width = Math.Max(0, ProgressTrack.ActualWidth * Math.Clamp(ratio, 0, 1));
            ProgressTrack.Visibility = Visibility.Visible;
            MediaDuration.Text = FormatTime(total);
        }
        else
        {
            // 不知道总时长：只显示已经播放了多久
            ProgressTrack.Visibility = Visibility.Hidden;
            MediaDuration.Text = "";
        }
        MediaPosition.Text = FormatTime(position);
    }

    private void OnProgressTrackSizeChanged(object sender, SizeChangedEventArgs e) => UpdateProgress();

    private static string FormatTime(TimeSpan t) =>
        t.TotalHours >= 1 ? t.ToString(@"h\:mm\:ss") : t.ToString(@"m\:ss");

    // ───────────────────────────── 动画开关 ─────────────────────────────

    /// <summary>律动条和进度刷新只在对应视图可见且正在播放时运行，空闲时不占 CPU。</summary>
    private void RefreshMediaAnimations()
    {
        bool playing = _media?.IsPlaying == true;

        bool musicVisible = ReferenceEquals(_currentView, ExpandedView) && _activeTab == IslandTab.Music;
        bool runProgress = playing && musicVisible;
        if (runProgress)
        {
            UpdateProgress();
            if (!_progressTimer.IsEnabled) _progressTimer.Start();
        }
        else
        {
            _progressTimer.Stop();
        }

        SetEqualizer(playing && ReferenceEquals(_currentView, MediaCompactView));
    }

    private void SetEqualizer(bool running)
    {
        if (running == _equalizerRunning) return;
        _equalizerRunning = running;

        int[] periods = [430, 560, 370, 500];
        double[] peaks = [0.95, 0.7, 1.0, 0.8];

        for (int i = 0; i < EqualizerBars.Length; i++)
        {
            if (EqualizerBars[i].RenderTransform is not ScaleTransform scale) continue;

            if (!running)
            {
                scale.BeginAnimation(ScaleTransform.ScaleYProperty, null);
                scale.ScaleY = 0.3;
                continue;
            }

            var bounce = new DoubleAnimation(0.25, peaks[i], TimeSpan.FromMilliseconds(periods[i]))
            {
                AutoReverse = true,
                RepeatBehavior = RepeatBehavior.Forever,
                BeginTime = TimeSpan.FromMilliseconds(i * 90),
                EasingFunction = new SineEase { EasingMode = EasingMode.EaseInOut },
            };
            Timeline.SetDesiredFrameRate(bounce, 30); // 律动条不需要 60 帧
            scale.BeginAnimation(ScaleTransform.ScaleYProperty, bounce);
        }
    }

    // ───────────────────────────── 按钮 ─────────────────────────────

    private void OnPlayPauseClick(object sender, RoutedEventArgs e) => _ = _mediaService?.TogglePlayPauseAsync();

    private void OnPreviousClick(object sender, RoutedEventArgs e) => _ = _mediaService?.PreviousAsync();

    private void OnNextClick(object sender, RoutedEventArgs e) => _ = _mediaService?.NextAsync();
}
