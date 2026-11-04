using Island.Core;
using Microsoft.Win32;

namespace Island.Views;

/// <summary>
/// 外观：主题（黑夜 / 白天 / 跟随系统 / 按时间 / 自定义）和待机样式。
/// 颜色本身由 <see cref="ThemeManager"/> 写入资源，这里决定什么时候用哪套颜色。
/// </summary>
public partial class IslandWindow
{
    private void InitializeTheme()
    {
        App.Theme.Changed += OnThemeChanged;
        SystemEvents.UserPreferenceChanged += OnUserPreferenceChanged;

        AppearancePanel.Attach(_settings, OnAppearanceChanged);
        ApplyTheme(animate: false);
        OnThemeChanged(App.Theme.Current);
    }

    private void ShutdownTheme()
    {
        App.Theme.Changed -= OnThemeChanged;
        SystemEvents.UserPreferenceChanged -= OnUserPreferenceChanged;
    }

    /// <summary>按当前设置、时间和系统偏好选出配色并应用（配色没变时什么也不做）。</summary>
    private void ApplyTheme(bool animate)
    {
        ThemePalette palette = ThemeResolver.Resolve(_settings.Theme, DateTime.Now, ThemeManager.SystemPrefersLight());
        App.Theme.Apply(palette, animate);
    }

    private void OnThemeChanged(ThemePalette palette)
    {
        // 浅色胶囊用更淡的阴影，否则显得脏
        CapsuleShadow.Opacity = palette.IsLight ? 0.16 : 0.35;
        CapsuleShadow.BlurRadius = palette.IsLight ? 14 : 18;
        // 音乐主题色按新背景重新调整可读性
        ApplyAccent(_rawAccent);
    }

    private void OnAppearanceChanged()
    {
        _settings.Save();
        ApplyTheme(animate: true);
        ApplyIdleStyle();
    }

    /// <summary>Windows 切换了浅色/深色应用模式。</summary>
    private void OnUserPreferenceChanged(object sender, UserPreferenceChangedEventArgs e)
    {
        if (e.Category != UserPreferenceCategory.General || _settings.Theme.Mode != ThemeMode.System) return;
        Dispatcher.BeginInvoke(new Action(() => ApplyTheme(animate: true)));
    }

    /// <summary>每分钟由时钟调用：“按时间”模式到点切换。</summary>
    private void CheckScheduledTheme()
    {
        if (_settings.Theme.Mode == ThemeMode.Schedule) ApplyTheme(animate: true);
    }
}
