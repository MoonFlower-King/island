using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;
using System.Windows.Media;
using Island.Core;

namespace Island.Views.Panels;

/// <summary>外观设置：主题模式（黑夜 / 白天 / 跟随系统 / 按时间 / 自定义）和待机样式。</summary>
public partial class AppearancePanel : UserControl
{
    private static readonly string[] BackgroundPresets =
        ["#000000", "#1C1C1E", "#0B1F3A", "#2A1B47", "#F7F7F8", "#FFF4DE"];

    private static readonly string[] ForegroundPresets =
        ["#FFFFFF", "#E5E5EA", "#FFD60A", "#1C1C1E", "#3A2E1F", "#0B1F3A"];

    private static readonly string[] AccentPresets =
        ["#0A84FF", "#30D158", "#FF9F0A", "#FF375F", "#BF5AF2", "#64D2FF"];

    private AppSettings? _settings;
    private Action? _changed;
    private bool _loading;

    public AppearancePanel() => InitializeComponent();

    /// <param name="changed">设置被修改后调用：窗口负责保存并应用。</param>
    public void Attach(AppSettings settings, Action changed)
    {
        _settings = settings;
        _changed = changed;

        AddSwatches(BackgroundSwatches, BackgroundPresets, hex => Settings.CustomBackground = hex);
        AddSwatches(ForegroundSwatches, ForegroundPresets, hex => Settings.CustomForeground = hex);
        AddSwatches(AccentSwatches, AccentPresets, hex => Settings.CustomAccent = hex);

        Refresh();
    }

    private ThemeSettings Settings => _settings!.Theme;

    /// <summary>把设置显示到界面上。</summary>
    public void Refresh()
    {
        if (_settings is null) return;
        _loading = true;

        RadioButton mode = Settings.Mode switch
        {
            ThemeMode.Light => ModeLight,
            ThemeMode.System => ModeSystem,
            ThemeMode.Schedule => ModeSchedule,
            ThemeMode.Custom => ModeCustom,
            _ => ModeDark,
        };
        mode.IsChecked = true;
        (_settings.IdleAsLine ? IdleLine : IdleClock).IsChecked = true;

        DayStartsInput.Text = Settings.DayStarts;
        NightStartsInput.Text = Settings.NightStarts;
        BackgroundInput.Text = Settings.CustomBackground;
        ForegroundInput.Text = Settings.CustomForeground;
        AccentInput.Text = Settings.CustomAccent;

        ScheduleRow.Visibility = Settings.Mode == ThemeMode.Schedule ? Visibility.Visible : Visibility.Collapsed;
        CustomRows.Visibility = Settings.Mode == ThemeMode.Custom ? Visibility.Visible : Visibility.Collapsed;
        Hint.Text = DescribeMode();

        _loading = false;
    }

    private string DescribeMode()
    {
        switch (Settings.Mode)
        {
            case ThemeMode.Light:
                return "浅色胶囊，适合浅色桌面和网页";
            case ThemeMode.System:
                return "跟随 Windows 设置 → 个性化 → 颜色 → 应用模式";
            case ThemeMode.Schedule:
                return "到点自动切换";
            case ThemeMode.Custom:
                if (ColorMath.TryParseHex(Settings.CustomBackground, out var bg) &&
                    ColorMath.TryParseHex(Settings.CustomForeground, out var fg) &&
                    ColorMath.ContrastRatio(bg, fg) < 3)
                    return "⚠ 文字和背景颜色太接近，可能看不清";
                return "点色块或输入颜色值，岛会立刻变成预览效果";
            default:
                return "黑色的经典灵动岛";
        }
    }

    private void AddSwatches(Panel host, string[] presets, Action<string> apply)
    {
        host.Children.Clear();
        foreach (string hex in presets)
        {
            if (!ColorMath.TryParseHex(hex, out Color color)) continue;
            var brush = new SolidColorBrush(color);
            brush.Freeze();

            var swatch = new Button { Style = (Style)FindResource("IslandSwatch"), Background = brush, ToolTip = hex };
            swatch.Click += (_, _) =>
            {
                apply(hex);
                Changed();
            };
            host.Children.Add(swatch);
        }
    }

    private void Changed()
    {
        _changed?.Invoke();
        Refresh();
    }

    private void OnModeChecked(object sender, RoutedEventArgs e)
    {
        if (_loading || _settings is null || sender is not RadioButton { Tag: string tag }) return;
        if (!Enum.TryParse(tag, out ThemeMode mode) || mode == Settings.Mode) return;
        Settings.Mode = mode;
        Changed();
    }

    private void OnIdleStyleChecked(object sender, RoutedEventArgs e)
    {
        if (_loading || _settings is null) return;
        bool line = ReferenceEquals(sender, IdleLine);
        if (line == _settings.IdleAsLine) return;
        _settings.IdleAsLine = line;
        Changed();
    }

    private void OnInputKeyDown(object sender, KeyEventArgs e)
    {
        if (e.Key != Key.Enter || sender is not TextBox box) return;
        e.Handled = true;
        if (box == DayStartsInput || box == NightStartsInput) CommitSchedule();
        else CommitHex(box);
    }

    private void OnScheduleCommitted(object sender, KeyboardFocusChangedEventArgs e) => CommitSchedule();

    private void OnHexCommitted(object sender, KeyboardFocusChangedEventArgs e)
    {
        if (sender is TextBox box) CommitHex(box);
    }

    private void CommitSchedule()
    {
        if (_loading || _settings is null) return;

        if (!ThemeResolver.TryParseTime(DayStartsInput.Text, out var day) ||
            !ThemeResolver.TryParseTime(NightStartsInput.Text, out var night))
        {
            Hint.Text = "时间格式是 时:分，例如 07:00";
            return;
        }

        string dayText = day.ToString(@"hh\:mm"), nightText = night.ToString(@"hh\:mm");
        if (dayText == Settings.DayStarts && nightText == Settings.NightStarts) return;
        Settings.DayStarts = dayText;
        Settings.NightStarts = nightText;
        Changed();
    }

    private void CommitHex(TextBox box)
    {
        if (_loading || _settings is null) return;

        if (!ColorMath.TryParseHex(box.Text, out Color color))
        {
            Hint.Text = "颜色格式是 #RRGGBB，例如 #1C1C1E";
            return;
        }

        string hex = ColorMath.ToHex(color);
        if (box == BackgroundInput) Settings.CustomBackground = hex;
        else if (box == ForegroundInput) Settings.CustomForeground = hex;
        else Settings.CustomAccent = hex;
        Changed();
    }
}
