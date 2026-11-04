using System.Windows;
using System.Windows.Media;
using System.Windows.Media.Animation;
using System.Windows.Shapes;
using System.Windows.Threading;
using Island.Media;

namespace Island.Views;

/// <summary>
/// 岛上的“正在播放”：
///   · 播放中 → 紧凑胶囊：封面 + 歌名 + 律动条
///   · 悬停   → 展开播放器：封面、歌曲信息、进度条、上一首/播放暂停/下一首
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

    private MediaSessionService? _mediaService;
    private MediaSnapshot? _media;
    private bool _mediaLingering;
    private bool _equalizerRunning;

    private bool HasMedia => _media is not null;

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

        MediaTimeline.Visibility = m.HasTimeline ? Visibility.Visible : Visibility.Hidden;
        UpdateProgress();
    }

    private static void SetArt(System.Windows.Controls.Border host, UIElement placeholder, ImageSource? art)
    {
        if (art is null)
        {
            host.Background = new SolidColorBrush(Color.FromRgb(0x2C, 0x2C, 0x2E));
            placeholder.Visibility = Visibility.Visible;
            return;
        }

        var brush = new ImageBrush(art) { Stretch = Stretch.UniformToFill };
        brush.Freeze();
        host.Background = brush;
        placeholder.Visibility = Visibility.Collapsed;
    }

    private void ApplyAccent(Color color)
    {
        var brush = new SolidColorBrush(color);
        brush.Freeze();
        ProgressFill.Background = brush;
        foreach (var bar in EqualizerBars) bar.Fill = brush;
    }

    // ───────────────────────────── 进度 ─────────────────────────────

    private void UpdateProgress()
    {
        if (_media is not { HasTimeline: true } m) return;

        TimeSpan position = m.EstimatePosition(DateTimeOffset.Now);
        double ratio = m.Duration.Ticks > 0 ? (double)position.Ticks / m.Duration.Ticks : 0;

        ProgressFill.Width = Math.Max(0, ProgressTrack.ActualWidth * Math.Clamp(ratio, 0, 1));
        MediaPosition.Text = FormatTime(position);
        MediaDuration.Text = FormatTime(m.Duration);
    }

    private void OnProgressTrackSizeChanged(object sender, SizeChangedEventArgs e) => UpdateProgress();

    private static string FormatTime(TimeSpan t) =>
        t.TotalHours >= 1 ? t.ToString(@"h\:mm\:ss") : t.ToString(@"m\:ss");

    // ───────────────────────────── 动画开关 ─────────────────────────────

    /// <summary>律动条和进度刷新只在对应视图可见且正在播放时运行，空闲时不占 CPU。</summary>
    private void RefreshMediaAnimations()
    {
        bool playing = _media?.IsPlaying == true;

        bool runProgress = playing && _media!.HasTimeline && ReferenceEquals(_currentView, MediaExpandedView);
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
