using System.IO;
using System.Windows;
using System.Windows.Media;
using System.Windows.Media.Imaging;
using System.Windows.Threading;
using Island.Core;

namespace Island.Views;

/// <summary>
/// 冒烟测试：Island.exe --smoke-test &lt;输出目录&gt;
///
/// 在真实窗口上走一遍所有状态和功能标签，检查没有崩溃、状态切换正确、动画最终收敛，
/// 并把每个界面渲染成 PNG。结果写到输出目录的 report.txt，全部通过时退出码为 0。
/// CI 在每次构建后都会运行它。
/// </summary>
public partial class IslandWindow
{
    private static readonly TimeSpan SmokeTimeout = TimeSpan.FromSeconds(90);

    internal async Task RunSmokeTestAsync(string outputDirectory)
    {
        var report = new List<string>();
        int failures = 0;

        void Check(bool ok, string what)
        {
            report.Add($"{(ok ? "PASS" : "FAIL")}  {what}");
            if (!ok) failures++;
        }

        void Note(string what) => report.Add($"INFO  {what}");

        // 防止某一步卡死导致 CI 一直挂着
        var watchdog = new DispatcherTimer { Interval = SmokeTimeout };
        watchdog.Tick += (_, _) =>
        {
            report.Add("FAIL  超时");
            File.WriteAllLines(Path.Combine(outputDirectory, "report.txt"), report);
            Application.Current.Shutdown(2);
        };
        watchdog.Start();

        try
        {
            Directory.CreateDirectory(outputDirectory);
            _autoHideForFullscreen = false;
            if (_hiddenForFullscreen) SetHiddenForFullscreen(false);
            Note($"数据目录：{JsonStore.DataDirectory}");

            await Settle(1300);
            Check(_currentState == IslandState.Idle, "启动后处于待机状态");
            CheckSize(IslandState.Idle, Check);
            Snapshot(outputDirectory, "01-idle");

            // 通知
            ShowNotification("冒烟测试", "这是一条测试通知", "💬");
            await Settle(1300);
            Check(_currentState == IslandState.Notification, "显示通知");
            CheckSize(IslandState.Notification, Check);
            Snapshot(outputDirectory, "02-notification");
            DismissNotification();
            await Settle(1300);
            Check(_currentState == IslandState.Idle, "关闭通知后回到待机");

            // 紧凑
            SetBaseState(IslandState.Compact, "冒烟测试进行中");
            await Settle(1300);
            Check(_currentState == IslandState.Compact && ReferenceEquals(_currentView, CompactView), "紧凑状态");
            CheckSize(IslandState.Compact, Check);
            Snapshot(outputDirectory, "03-compact");
            SetBaseState(IslandState.Idle);
            await Settle(1300);

            // 展开并逐个打开功能标签
            _holdExpanded = true;
            Expand();
            await Settle(1300);
            Check(_currentState == IslandState.Expanded && ReferenceEquals(_currentView, ExpandedView), "展开");
            CheckSize(IslandState.Expanded, Check);

            int index = 4;
            foreach (IslandTab tab in Enum.GetValues<IslandTab>())
            {
                ShowTab(tab, remember: false);
                await Settle(tab == IslandTab.Monitor ? 2500 : 400);
                FrameworkElement panel = PanelFor(tab);
                Check(panel.Visibility == Visibility.Visible && panel.ActualHeight > 50 && panel.ActualWidth > 200,
                    $"标签 {tab} 可见（{panel.ActualWidth:0}×{panel.ActualHeight:0}）");
                Snapshot(outputDirectory, $"{index++:00}-tab-{tab.ToString().ToLowerInvariant()}");
            }
            Check(MonitorPanel.HasData, "系统监控有采样数据");

            // 待办：添加、勾选后沉底、删除
            ShowTab(IslandTab.Todo, remember: false);
            _todo!.Add("冒烟测试：第一项");
            _todo.Add("冒烟测试：第二项");
            await Settle(300);
            Check(_todo.Items.Count >= 2 && _todo.Items[0].Text == "冒烟测试：第二项", "添加待办（新的在最前）");
            var first = _todo.Items[0];
            first.Done = true;
            await Settle(300);
            Check(ReferenceEquals(_todo.Items[^1], first), "完成的待办沉到底部");
            Snapshot(outputDirectory, $"{index++:00}-todo-items");
            _todo.ClearCompleted();
            await Settle(200);
            Check(!_todo.Items.Contains(first), "清除已完成");

            // 剪贴板：CI 环境的剪贴板不一定可用，失败只记录不算错
            string marker = $"island smoke {Guid.NewGuid():N}";
            try
            {
                Clipboard.SetText(marker);
                await Settle(1300);
                bool recorded = _clipboard!.Items.FirstOrDefault()?.Text == marker;
                Note(recorded ? "剪贴板历史记录到了新复制的文字" : "剪贴板历史没有记录到（CI 环境可能不支持）");
                ShowTab(IslandTab.Clipboard, remember: false);
                await Settle(300);
                Snapshot(outputDirectory, $"{index++:00}-clipboard-items");
            }
            catch (Exception ex)
            {
                Note($"剪贴板不可用：{ex.Message}");
            }

            // 中转站
            string sample = Path.Combine(outputDirectory, "shelf-sample.txt");
            await File.WriteAllTextAsync(sample, "hello");
            _shelf!.Add([sample, outputDirectory]);
            ShowTab(IslandTab.Shelf, remember: false);
            await Settle(300);
            Check(_shelf.Items.Count >= 2 && _shelf.Items.Any(i => i.Path == sample), "文件加入中转站");
            Snapshot(outputDirectory, $"{index++:00}-shelf-items");
            _shelf.Clear();

            // 主题：白天、自定义，再回到黑夜；颜色资源应随之改变
            ShowTab(IslandTab.Music, remember: false);
            ThemeMode originalMode = _settings.Theme.Mode;
            _settings.Theme.Mode = ThemeMode.Light;
            ApplyTheme(animate: false);
            await Settle(300);
            Check(App.Theme.Current == ThemePalette.Light && BrushColor("IslandBackground") == ThemePalette.Light.Background,
                "切换到白天主题");
            Snapshot(outputDirectory, $"{index++:00}-theme-light");

            _settings.Theme.Mode = ThemeMode.Custom;
            _settings.Theme.CustomBackground = "#0B1F3A";
            _settings.Theme.CustomForeground = "#FFD60A";
            _settings.Theme.CustomAccent = "#FF375F";
            ApplyTheme(animate: true);
            await Settle(600);
            Check(BrushColor("IslandBackground") == Color.FromRgb(0x0B, 0x1F, 0x3A) &&
                  BrushColor("IslandForeground") == Color.FromRgb(0xFF, 0xD6, 0x0A), "自定义主题（含颜色过渡动画）");
            ShowTab(IslandTab.Appearance, remember: false);
            AppearancePanel.Refresh();
            await Settle(300);
            Snapshot(outputDirectory, $"{index++:00}-theme-custom");

            _settings.Theme.Mode = originalMode;
            ApplyTheme(animate: false);
            Check(BrushColor("IslandBackground") == ThemePalette.Dark.Background, "回到黑夜主题");

            // 收起
            _holdExpanded = false;
            TryCollapse();
            await Settle(1300);
            Check(_currentState == IslandState.Idle, "收起后回到待机");
            CheckSize(IslandState.Idle, Check);
            Check(ReferenceEquals(_currentView, LineView) == IdleAsLine, "待机样式与设置一致");

            // 待机样式：切到“显示时间”再切回细线
            bool originalIdle = _settings.IdleAsLine;
            _settings.IdleAsLine = !originalIdle;
            ApplyIdleStyle();
            await Settle(1300);
            CheckSize(IslandState.Idle, Check);
            Snapshot(outputDirectory, $"{index++:00}-idle-{(IdleAsLine ? "line" : "clock")}");
            _settings.IdleAsLine = originalIdle;
            ApplyIdleStyle();
            await Settle(1300);
            CheckSize(IslandState.Idle, Check);

            // 快速反复展开收起：动画应能被打断并最终收敛，不会卡在中间尺寸
            for (int i = 0; i < 20; i++)
            {
                _holdExpanded = i % 2 == 0;
                if (_holdExpanded) Expand();
                else TryCollapse();
                await Task.Delay(35);
            }
            _holdExpanded = false;
            TryCollapse();
            await Settle(1300);
            Check(_currentState == IslandState.Idle, "快速切换后回到待机");
            CheckSize(IslandState.Idle, Check);
            Snapshot(outputDirectory, $"{index:00}-idle-after-stress");
        }
        catch (Exception ex)
        {
            report.Add($"FAIL  异常：{ex}");
            failures++;
        }

        watchdog.Stop();
        report.Add(failures == 0 ? "RESULT  全部通过" : $"RESULT  {failures} 项失败");
        File.WriteAllLines(Path.Combine(outputDirectory, "report.txt"), report);
        Log.Info($"冒烟测试结束：{failures} 项失败");
        Application.Current.Shutdown(failures == 0 ? 0 : 1);
    }

    private static Task Settle(int milliseconds) => Task.Delay(milliseconds);

    private static Color BrushColor(string key) => App.Theme.ColorOf(key);

    /// <summary>动画结束后胶囊尺寸应与该状态的设计尺寸一致。</summary>
    private void CheckSize(IslandState state, Action<bool, string> check)
    {
        IslandMetrics m = MetricsFor(state);
        Rect r = _shape.CurrentRect;
        check(!_shape.IsAnimating && Math.Abs(r.Width - m.Width) < 0.5 && Math.Abs(r.Height - m.Height) < 0.5 &&
              Math.Abs(r.Top - m.Top) < 0.5,
            $"{state} 形状 {r.Width:0.#}×{r.Height:0.#} @ {r.Top:0.#}（期望 {m.Width}×{m.Height} @ {m.Top}）");
    }

    /// <summary>把整个窗口画在灰色背景上存成 PNG（窗口本身是透明的）。</summary>
    private void Snapshot(string directory, string name)
    {
        const double scale = 2;
        int width = (int)Math.Ceiling(ActualWidth * scale);
        int height = (int)Math.Ceiling(ActualHeight * scale);

        var background = new DrawingVisual();
        using (DrawingContext dc = background.RenderOpen())
            dc.DrawRectangle(new SolidColorBrush(Color.FromRgb(0x3A, 0x3F, 0x4B)), null,
                new Rect(0, 0, ActualWidth, ActualHeight));

        // RenderTargetBitmap 会叠加多次 Render 的结果：先画背景，再画岛
        var bitmap = new RenderTargetBitmap(width, height, 96 * scale, 96 * scale, PixelFormats.Pbgra32);
        bitmap.Render(background);
        bitmap.Render(Root);

        var encoder = new PngBitmapEncoder();
        encoder.Frames.Add(BitmapFrame.Create(bitmap));
        using FileStream file = File.Create(Path.Combine(directory, name + ".png"));
        encoder.Save(file);
    }
}
