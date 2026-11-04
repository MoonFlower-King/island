using System.Diagnostics;
using System.IO;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Controls.Primitives;
using System.Windows.Input;
using System.Windows.Media;
using System.Windows.Media.Animation;
using System.Windows.Media.Media3D;
using Island.Core;
using Island.Interop;
using Island.Services;

namespace Island.Views;

/// <summary>
/// 展开面板里的六个功能：音乐、待办、剪贴板历史、文件中转站、AI 翻译、系统监控。
/// 以及它们需要的窗口级能力：键盘输入、拖放文件、监听剪贴板。
/// </summary>
public partial class IslandWindow
{
    private readonly System.Windows.Threading.DispatcherTimer _dragLeaveTimer =
        new() { Interval = TimeSpan.FromMilliseconds(150) };

    private AppSettings _settings = new();
    private TodoService? _todo;
    private ClipboardService? _clipboard;
    private ShelfService? _shelf;

    private IslandTab _activeTab = IslandTab.Music;
    private bool _suppressTabEvent;
    private bool _isDraggingOver;
    private bool _isTyping;
    private IntPtr _previousForeground;

    private IslandTab PreferredTab =>
        Enum.TryParse(_settings.LastTab, out IslandTab tab) ? tab : IslandTab.Music;

    private void InitializeFeatures()
    {
        _todo = new TodoService();
        _clipboard = new ClipboardService(_settings);
        _shelf = new ShelfService();

        TodoPanel.PendingCountChanged += count =>
            TabTodo.ToolTip = count > 0 ? $"待办（{count} 项未完成）" : "待办";
        TodoPanel.Attach(_todo);
        ClipboardPanel.Attach(_clipboard);
        ShelfPanel.Attach(_shelf);
        TranslatePanel.Attach(new TranslationService(_settings));

        PersistClipboardMenu.IsChecked = _settings.PersistClipboardHistory;
        StartupMenu.IsChecked = StartupRegistration.IsEnabled;

        _dragLeaveTimer.Tick += (_, _) =>
        {
            _dragLeaveTimer.Stop();
            _isDraggingOver = false;
            TryCollapse();
        };

        ShowTab(PreferredTab, remember: false);
    }

    private void ShutdownFeatures()
    {
        _dragLeaveTimer.Stop();
        MonitorPanel.SetActive(false);
        _todo?.Flush();
        _clipboard?.Flush();
    }

    // ───────────────────────────── 标签切换 ─────────────────────────────

    private RadioButton TabFor(IslandTab tab) => tab switch
    {
        IslandTab.Music => TabMusic,
        IslandTab.Todo => TabTodo,
        IslandTab.Clipboard => TabClipboard,
        IslandTab.Shelf => TabShelf,
        IslandTab.Translate => TabTranslate,
        IslandTab.Monitor => TabMonitor,
        IslandTab.Appearance => TabAppearance,
        _ => TabMusic,
    };

    private FrameworkElement PanelFor(IslandTab tab) => tab switch
    {
        IslandTab.Music => MusicPanel,
        IslandTab.Todo => TodoPanel,
        IslandTab.Clipboard => ClipboardPanel,
        IslandTab.Shelf => ShelfPanel,
        IslandTab.Translate => TranslatePanel,
        IslandTab.Monitor => MonitorPanel,
        IslandTab.Appearance => AppearancePanel,
        _ => MusicPanel,
    };

    private void OnTabChecked(object sender, RoutedEventArgs e)
    {
        if (_suppressTabEvent || sender is not RadioButton { Tag: string name }) return;
        if (!Enum.TryParse(name, out IslandTab tab)) return;

        ShowTab(tab, remember: true);

        // 用户主动点了需要打字的标签：直接进入输入状态
        if (tab == IslandTab.Todo)
        {
            BeginTyping();
            TodoPanel.FocusInput();
        }
        else if (tab == IslandTab.Translate)
        {
            BeginTyping();
            TranslatePanel.FocusInput();
        }
    }

    /// <param name="remember">是否记为“上次用的标签”。自动切换（拖文件、从音乐胶囊展开）不记。</param>
    private void ShowTab(IslandTab tab, bool remember)
    {
        bool changed = tab != _activeTab;
        _activeTab = tab;

        _suppressTabEvent = true;
        TabFor(tab).IsChecked = true;
        _suppressTabEvent = false;

        foreach (IslandTab t in Enum.GetValues<IslandTab>())
        {
            FrameworkElement panel = PanelFor(t);
            bool visible = t == tab;
            panel.Visibility = visible ? Visibility.Visible : Visibility.Collapsed;

            if (visible && changed)
            {
                var fade = new DoubleAnimation(0, 1, TimeSpan.FromMilliseconds(140))
                {
                    EasingFunction = new CubicEase { EasingMode = EasingMode.EaseOut },
                };
                panel.BeginAnimation(OpacityProperty, fade);
            }
        }

        if (remember && _settings.LastTab != tab.ToString())
        {
            _settings.LastTab = tab.ToString();
            _settings.Save();
        }

        UpdatePanelActivity();
    }

    /// <summary>只有展开且正在看的面板才需要刷新（监控采样、音乐进度）。</summary>
    private void UpdatePanelActivity()
    {
        bool expanded = ReferenceEquals(_currentView, ExpandedView);
        MonitorPanel.SetActive(expanded && _activeTab == IslandTab.Monitor);
        RefreshMediaAnimations();
    }

    // ───────────────────────────── 键盘输入 ─────────────────────────────
    //
    // 岛默认不抢焦点（WS_EX_NOACTIVATE），这样点它不会打断别的程序。
    // 但待办和翻译需要打字，所以点到输入框时临时激活窗口，收起时把焦点还给原来的程序。

    private void OnCapsulePreviewMouseDown(object sender, MouseButtonEventArgs e)
    {
        if (IsInsideTextBox(e.OriginalSource as DependencyObject)) BeginTyping();
    }

    private static bool IsInsideTextBox(DependencyObject? node)
    {
        while (node is not null)
        {
            if (node is TextBoxBase) return true;
            node = node is Visual or Visual3D ? VisualTreeHelper.GetParent(node) : LogicalTreeHelper.GetParent(node);
        }
        return false;
    }

    private void BeginTyping()
    {
        _isTyping = true;
        if (IsActive) return;

        IntPtr foreground = NativeMethods.GetForegroundWindow();
        if (foreground != _hwnd) _previousForeground = foreground;

        NativeMethods.SetForegroundWindow(_hwnd);
        Activate();
    }

    private void EndTyping(bool restoreFocus)
    {
        if (!_isTyping) return;
        _isTyping = false;
        Keyboard.ClearFocus();

        if (restoreFocus && IsActive && _previousForeground != IntPtr.Zero && NativeMethods.IsWindow(_previousForeground))
            NativeMethods.SetForegroundWindow(_previousForeground);
        _previousForeground = IntPtr.Zero;
    }

    private void OnWindowPreviewKeyDown(object sender, KeyEventArgs e)
    {
        // Esc：收起岛，焦点还给之前的程序
        if (e.Key != Key.Escape) return;
        e.Handled = true;
        EndTyping(restoreFocus: true);
        _collapseWatchdog.Stop();
        _isHovering = false;
        UpdateState();
    }

    private void OnWindowDeactivated(object? sender, EventArgs e)
    {
        // 用户点了别的地方：不再保持输入状态，鼠标也不在岛上就收起
        _isTyping = false;
        _previousForeground = IntPtr.Zero;
        TryCollapse();
    }

    // ───────────────────────────── 拖放文件 → 中转站 ─────────────────────────────

    private void OnIslandDragOver(object sender, DragEventArgs e)
    {
        e.Handled = true;
        if (!e.Data.GetDataPresent(DataFormats.FileDrop))
        {
            e.Effects = DragDropEffects.None;
            return;
        }

        e.Effects = DragDropEffects.Copy;
        _dragLeaveTimer.Stop();
        _hoverOutTimer.Stop();

        if (!_isDraggingOver)
        {
            _isDraggingOver = true;
            Expand(IslandTab.Shelf);
        }
    }

    private void OnIslandDragLeave(object sender, DragEventArgs e)
    {
        // 在子元素之间移动也会触发 DragLeave，稍等确认真的离开了
        _dragLeaveTimer.Stop();
        _dragLeaveTimer.Start();
    }

    private void OnIslandDrop(object sender, DragEventArgs e)
    {
        e.Handled = true;
        _dragLeaveTimer.Stop();
        _isDraggingOver = false;

        if (e.Data.GetData(DataFormats.FileDrop) is string[] paths && paths.Length > 0)
        {
            _shelf?.Add(paths);
            ShowTab(IslandTab.Shelf, remember: false);
        }
    }

    // ───────────────────────────── 剪贴板 ─────────────────────────────

    private void OnClipboardUpdated() => _clipboard?.OnClipboardUpdated();

    private void OnTogglePersistClipboard(object sender, RoutedEventArgs e)
    {
        _settings.PersistClipboardHistory = PersistClipboardMenu.IsChecked;
        _settings.Save();

        if (_settings.PersistClipboardHistory)
        {
            _clipboard?.SaveNow();
        }
        else
        {
            // 关闭后删除磁盘上的历史，内存里的保留到退出
            try
            {
                File.Delete(JsonStore.PathOf("clipboard.json"));
            }
            catch (Exception ex)
            {
                Debug.WriteLine($"[Clipboard] 删除历史文件失败：{ex.Message}");
            }
        }
    }

    private void OnToggleStartup(object sender, RoutedEventArgs e)
    {
        bool wanted = StartupMenu.IsChecked;
        if (!StartupRegistration.SetEnabled(wanted)) StartupMenu.IsChecked = !wanted;
    }

    private void OnOpenDataFolder(object sender, RoutedEventArgs e)
    {
        try
        {
            Directory.CreateDirectory(JsonStore.DataDirectory);
            Process.Start(new ProcessStartInfo("explorer.exe", $"\"{JsonStore.DataDirectory}\"") { UseShellExecute = true });
        }
        catch (Exception ex)
        {
            Debug.WriteLine($"[Island] 打开数据文件夹失败：{ex.Message}");
        }
    }
}
