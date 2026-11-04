using System.Windows;
using System.Windows.Input;
using System.Windows.Media;
using System.Windows.Media.Animation;
using System.Windows.Threading;
using Island.Core;
using Island.Services;

namespace Island.Views;

/// <summary>
/// 音量 / 亮度提示：
///   · 任何方式改了系统音量（键盘音量键、任务栏、其他程序）或笔记本屏幕亮度，岛短暂变成一条进度条；
///   · 在岛上滚动滚轮可以直接调音量（收起时、或展开在音乐标签时）；
///   · 音乐播放器里有一条可以点击 / 拖动的音量条。
/// </summary>
public partial class IslandWindow
{
    /// <summary>停手后提示停留多久再缩回。</summary>
    private static readonly TimeSpan LevelHudDuration = TimeSpan.FromMilliseconds(1600);

    private readonly DispatcherTimer _levelTimer = new() { Interval = LevelHudDuration };
    private VolumeService? _volume;
    private BrightnessService? _brightness;
    private bool _showingLevel;
    private bool _wheelMode;        // 用户在岛上滚滚轮调音量时，不要因为鼠标停在上面就展开
    private bool _draggingVolume;
    private DateTime _lastWheelAt;

    /// <summary>滚轮停下多久后，鼠标停在岛上又可以正常展开了。</summary>
    private static readonly TimeSpan WheelModeTimeout = TimeSpan.FromSeconds(1.2);

    /// <summary>刚刚在用滚轮调音量（防止 MouseLeave 漏掉时一直卡在滚轮模式）。</summary>
    private bool IsWheelActive()
    {
        if (_wheelMode && DateTime.UtcNow - _lastWheelAt > WheelModeTimeout) _wheelMode = false;
        return _wheelMode;
    }

    private void InitializeLevels()
    {
        LevelHudMenu.IsChecked = _settings.ShowLevelHud;
        _levelTimer.Tick += (_, _) =>
        {
            _levelTimer.Stop();
            _showingLevel = false;
            UpdateState();
        };

        _volume = new VolumeService(Dispatcher);
        _volume.Changed += OnVolumeChanged;
        _volume.Start();
        if (_volume.Current is { } current) UpdateMediaVolume(current);
        MediaVolumeRow.Visibility = _volume.IsAvailable ? Visibility.Visible : Visibility.Collapsed;

        _brightness = new BrightnessService(Dispatcher);
        _brightness.Changed += level => ShowLevel(LevelGlyphs.Brightness, level, LevelGlyphs.Percent(level, false));
        _brightness.Start();
    }

    private void ShutdownLevels()
    {
        _levelTimer.Stop();
        _volume?.Dispose();
        _brightness?.Dispose();
    }

    private void OnVolumeChanged(VolumeLevel volume)
    {
        MediaVolumeRow.Visibility = Visibility.Visible;
        UpdateMediaVolume(volume);
        ShowLevel(LevelGlyphs.ForVolume(volume.Level, volume.Muted), volume.Muted ? 0 : volume.Level,
            LevelGlyphs.Percent(volume.Level, volume.Muted));
    }

    /// <summary>显示（或刷新）提示，并从现在起重新计时。</summary>
    private void ShowLevel(string glyph, double level, string text)
    {
        // 展开着的时候不打断用户（音乐标签里自带音量条）
        if (!_settings.ShowLevelHud || _isHovering || _hiddenForFullscreen) return;

        LevelIcon.Text = glyph;
        LevelText.Text = text;
        var fill = new DoubleAnimation(Math.Clamp(level, 0, 1), TimeSpan.FromMilliseconds(140))
        {
            EasingFunction = new CubicEase { EasingMode = EasingMode.EaseOut },
        };
        Timeline.SetDesiredFrameRate(fill, 120);
        LevelFillScale.BeginAnimation(ScaleTransform.ScaleXProperty, fill, HandoffBehavior.SnapshotAndReplace);

        _showingLevel = true;
        _levelTimer.Stop();
        _levelTimer.Start();
        UpdateState();
    }

    private void UpdateMediaVolume(VolumeLevel volume)
    {
        MediaVolumeIcon.Text = LevelGlyphs.ForVolume(volume.Level, volume.Muted);
        MediaVolumeText.Text = LevelGlyphs.Percent(volume.Level, volume.Muted);
        MediaVolumeScale.ScaleX = volume.Muted ? 0 : Math.Clamp(volume.Level, 0, 1);
    }

    // ───────────────────────────── 滚轮调音量 ─────────────────────────────

    private void OnIslandMouseWheel(object sender, MouseWheelEventArgs e)
    {
        if (_volume is not { IsAvailable: true }) return;

        // 展开在别的标签时，滚轮留给列表滚动
        if (_isHovering && _activeTab != IslandTab.Music) return;

        if (!_isHovering)
        {
            // 鼠标停在细线上滚滚轮：用户想调音量，不是想打开面板
            _wheelMode = true;
            _hoverInTimer.Stop();
            _lastWheelAt = DateTime.UtcNow;
        }

        _volume.Nudge(e.Delta);
        e.Handled = true;
    }

    // ───────────────────────────── 播放器里的音量条 ─────────────────────────────

    private void OnVolumeTrackMouseDown(object sender, MouseButtonEventArgs e)
    {
        _draggingVolume = true;
        MediaVolumeTrack.CaptureMouse();
        SetVolumeFromPointer(e);
        e.Handled = true;
    }

    private void OnVolumeTrackMouseMove(object sender, MouseEventArgs e)
    {
        if (_draggingVolume && e.LeftButton == MouseButtonState.Pressed) SetVolumeFromPointer(e);
    }

    private void OnVolumeTrackMouseUp(object sender, MouseButtonEventArgs e)
    {
        _draggingVolume = false;
        MediaVolumeTrack.ReleaseMouseCapture();
        e.Handled = true;
    }

    private void SetVolumeFromPointer(MouseEventArgs e)
    {
        double width = MediaVolumeTrack.ActualWidth;
        if (width <= 0 || _volume is null) return;
        double level = Math.Clamp(e.GetPosition(MediaVolumeTrack).X / width, 0, 1);
        MediaVolumeScale.ScaleX = level; // 先更新界面，跟手
        _volume.SetLevel(level);
    }

    private void OnToggleLevelHud(object sender, RoutedEventArgs e)
    {
        _settings.ShowLevelHud = LevelHudMenu.IsChecked;
        _settings.Save();
        if (!_settings.ShowLevelHud && _showingLevel)
        {
            _levelTimer.Stop();
            _showingLevel = false;
            UpdateState();
        }
    }
}
