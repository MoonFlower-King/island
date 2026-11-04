using System.Globalization;
using System.Windows;
using System.Windows.Input;
using System.Windows.Interop;
using System.Windows.Media;
using System.Windows.Media.Animation;
using System.Windows.Threading;
using Island.Animation;
using Island.Core;
using Island.Interop;
using Microsoft.Win32;

namespace Island.Views;

/// <summary>
/// 灵动岛主窗口：一个透明、置顶、不抢焦点的窗口，中间放一个会变形的黑色胶囊。
///
/// 显示哪个状态由这几件事决定（优先级从高到低）：
///   1. 鼠标在岛上（或正拖着文件经过） → 展开
///   2. 有临时通知 → 通知
///   3. 否则 → 基础状态（有音乐在播放或手动设置时为紧凑，否则待机）
///
/// 同一个状态可能对应不同的内容视图（例如紧凑态放音乐时显示封面），见 <see cref="ViewFor"/>。
/// 各功能标签的逻辑在 IslandWindow.Features.cs，音乐在 IslandWindow.Media.cs。
/// </summary>
public partial class IslandWindow : Window
{
    private static readonly CultureInfo DateCulture = new("zh-CN");

    public static readonly DependencyProperty CapsuleRadiusProperty =
        DependencyProperty.Register(nameof(CapsuleRadius), typeof(double), typeof(IslandWindow),
            new PropertyMetadata(IslandMetrics.For(IslandState.Idle).CornerRadius,
                (d, _) => ((IslandWindow)d).ApplyCornerRadius()));

    /// <summary>胶囊圆角。WPF 没有 CornerRadius 动画，所以用一个 double 属性做动画再同步过去。</summary>
    public double CapsuleRadius
    {
        get => (double)GetValue(CapsuleRadiusProperty);
        set => SetValue(CapsuleRadiusProperty, value);
    }

    // 进入稍等一下再展开，避免鼠标只是路过顶部（比如去点浏览器标签）就弹开；离开则很快收起
    private readonly DispatcherTimer _hoverInTimer = new() { Interval = TimeSpan.FromMilliseconds(100) };
    private readonly DispatcherTimer _hoverOutTimer = new() { Interval = TimeSpan.FromMilliseconds(120) };
    // 展开期间定时确认鼠标还在岛上，防止快速甩出时漏掉 MouseLeave 导致不收起
    private readonly DispatcherTimer _collapseWatchdog = new() { Interval = TimeSpan.FromMilliseconds(200) };
    private readonly DispatcherTimer _notificationTimer = new();
    private readonly DispatcherTimer _clockTimer = new();
    // 定期抢回置顶，并检查是否有全屏程序（有就隐藏自己）
    private readonly DispatcherTimer _topmostTimer = new() { Interval = TimeSpan.FromSeconds(1.5) };

    private IntPtr _hwnd;
    private IslandState _baseState = IslandState.Idle;
    private IslandState _currentState = IslandState.Idle;
    private FrameworkElement? _currentView;
    private bool _isHovering;
    private bool _hasNotification;
    private bool _holdExpanded;           // 强制保持展开（冒烟测试用）
    private bool _autoHideForFullscreen = true;
    private bool _hiddenForFullscreen;

    public IslandWindow()
    {
        InitializeComponent();

        Width = IslandMetrics.WindowWidth;
        Height = IslandMetrics.WindowHeight;
        CapsuleHost.Margin = new Thickness(0, IslandMetrics.TopMargin, 0, 0);

        var idle = IslandMetrics.For(IslandState.Idle);
        CapsuleHost.Width = idle.Width;
        CapsuleHost.Height = idle.Height;

        foreach (var view in AllViews)
        {
            view.RenderTransformOrigin = new Point(0.5, 0.5);
            view.RenderTransform = new ScaleTransform(1, 1);
        }

        _hoverInTimer.Tick += (_, _) =>
        {
            _hoverInTimer.Stop();
            if (!IsCursorOverIsland()) return;
            Expand();
        };
        _hoverOutTimer.Tick += (_, _) =>
        {
            _hoverOutTimer.Stop();
            TryCollapse();
        };
        _collapseWatchdog.Tick += (_, _) => TryCollapse();
        _notificationTimer.Tick += (_, _) => DismissNotification();
        _clockTimer.Tick += (_, _) => UpdateClock();
        _topmostTimer.Tick += (_, _) => OnTopmostTick();

        SystemEvents.DisplaySettingsChanged += OnDisplaySettingsChanged;
        SystemParameters.StaticPropertyChanged += OnSystemParametersChanged;
        Closed += OnClosed;

        _currentView = IdleView;
        InitializeMedia();
        InitializeFeatures();

        PositionWindow();
        UpdateClock();
    }

    private FrameworkElement[] AllViews =>
        [IdleView, CompactView, MediaCompactView, NotificationView, ExpandedView];

    // ───────────────────────────── 窗口与 Win32 ─────────────────────────────

    protected override void OnSourceInitialized(EventArgs e)
    {
        base.OnSourceInitialized(e);

        _hwnd = new WindowInteropHelper(this).Handle;
        NativeMethods.MakeOverlay(_hwnd);
        HwndSource.FromHwnd(_hwnd)?.AddHook(WndProc);
        NativeMethods.AddClipboardFormatListener(_hwnd);

        _topmostTimer.Start();
    }

    private IntPtr WndProc(IntPtr hwnd, int msg, IntPtr wParam, IntPtr lParam, ref bool handled)
    {
        switch (msg)
        {
            // 点击岛时不要激活窗口，否则用户正在打字的窗口会失去焦点（需要打字时另行激活，见 BeginTyping）
            case NativeMethods.WM_MOUSEACTIVATE:
                handled = true;
                return new IntPtr(NativeMethods.MA_NOACTIVATE);

            case NativeMethods.WM_CLIPBOARDUPDATE:
                // 不在消息处理中直接读剪贴板，排到之后执行
                Dispatcher.BeginInvoke(new Action(OnClipboardUpdated));
                break;
        }
        return IntPtr.Zero;
    }

    private void OnTopmostTick()
    {
        if (_hwnd == IntPtr.Zero) return;

        bool fullscreen = _autoHideForFullscreen && NativeMethods.IsFullscreenAppRunning();
        if (fullscreen != _hiddenForFullscreen) SetHiddenForFullscreen(fullscreen);

        // 全屏时不去抢置顶，免得盖在视频或游戏上
        if (!fullscreen) NativeMethods.BringToTopmost(_hwnd);
    }

    /// <summary>看视频、打游戏、放 PPT 时淡出并让鼠标穿透，结束后恢复。</summary>
    private void SetHiddenForFullscreen(bool hidden)
    {
        _hiddenForFullscreen = hidden;
        Root.IsHitTestVisible = !hidden;
        Root.BeginAnimation(OpacityProperty, new DoubleAnimation(hidden ? 0 : 1, TimeSpan.FromMilliseconds(200)));

        if (hidden)
        {
            _hoverInTimer.Stop();
            _collapseWatchdog.Stop();
            _isHovering = false;
            EndTyping(restoreFocus: false);
            UpdateState();
        }
        Log.Info(hidden ? "检测到全屏程序，隐藏" : "全屏结束，恢复显示");
    }

    /// <summary>放在主显示器工作区的顶部正中。</summary>
    private void PositionWindow()
    {
        Rect workArea = SystemParameters.WorkArea;
        Left = workArea.Left + (workArea.Width - Width) / 2;
        Top = workArea.Top;
    }

    private void OnDisplaySettingsChanged(object? sender, EventArgs e) =>
        Dispatcher.BeginInvoke(new Action(PositionWindow));

    private void OnSystemParametersChanged(object? sender, System.ComponentModel.PropertyChangedEventArgs e)
    {
        if (e.PropertyName == nameof(SystemParameters.WorkArea))
            Dispatcher.BeginInvoke(new Action(PositionWindow));
    }

    private void OnClosed(object? sender, EventArgs e)
    {
        // SystemEvents 是静态事件，不取消订阅会泄漏
        SystemEvents.DisplaySettingsChanged -= OnDisplaySettingsChanged;
        SystemParameters.StaticPropertyChanged -= OnSystemParametersChanged;
        if (_hwnd != IntPtr.Zero) NativeMethods.RemoveClipboardFormatListener(_hwnd);

        foreach (var timer in new[]
                 {
                     _hoverInTimer, _hoverOutTimer, _collapseWatchdog, _notificationTimer, _clockTimer, _topmostTimer,
                 })
            timer.Stop();

        ShutdownMedia();
        ShutdownFeatures();
    }

    // ───────────────────────────── 状态机 ─────────────────────────────

    private IslandState ResolveState()
    {
        if (_isHovering) return IslandState.Expanded;
        if (_hasNotification) return IslandState.Notification;
        return _baseState == IslandState.Compact || MediaWantsCompact ? IslandState.Compact : IslandState.Idle;
    }

    private void UpdateState()
    {
        IslandState target = ResolveState();
        FrameworkElement view = ViewFor(target);
        if (target == _currentState && ReferenceEquals(view, _currentView)) return;

        IslandState previous = _currentState;
        _currentState = target;
        _currentView = view;

        if (previous == target)
            CrossFadeTo(view, growing: false); // 尺寸不变，只换内容
        else
            TransitionTo(previous, target, view);

        OnActiveViewChanged();
    }

    private void TransitionTo(IslandState from, IslandState to, FrameworkElement view)
    {
        IslandMetrics a = IslandMetrics.For(from);
        IslandMetrics b = IslandMetrics.For(to);
        bool growing = b.Width * b.Height > a.Width * a.Height;

        // 变大时有弹性，缩小时又快又稳——离开后要立刻让出屏幕
        var ease = growing
            ? new SpringEase { DampingRatio = 0.7, Stiffness = 11 }
            : new SpringEase { DampingRatio = 0.92, Stiffness = 12 };
        var duration = TimeSpan.FromMilliseconds(growing ? 480 : 300);

        Animate(CapsuleHost, WidthProperty, b.Width, duration, ease);
        Animate(CapsuleHost, HeightProperty, b.Height, duration, ease);
        Animate(this, CapsuleRadiusProperty, b.CornerRadius, duration, ease);

        CrossFadeTo(view, growing);
    }

    private FrameworkElement ViewFor(IslandState state) => state switch
    {
        IslandState.Idle => IdleView,
        IslandState.Compact => MediaWantsCompact ? MediaCompactView : CompactView,
        IslandState.Notification => NotificationView,
        IslandState.Expanded => ExpandedView,
        _ => IdleView,
    };

    /// <summary>可见内容变了：只让当前可见的视图跑动画和定时器，其余全部停掉。</summary>
    private void OnActiveViewChanged()
    {
        UpdateCompactPulse(ReferenceEquals(_currentView, CompactView));
        UpdatePanelActivity();
    }

    /// <summary>旧内容快速淡出，新内容等胶囊长出一点后再淡入并轻微放大。</summary>
    private void CrossFadeTo(FrameworkElement target, bool growing)
    {
        var fadeOut = TimeSpan.FromMilliseconds(80);
        var fadeIn = TimeSpan.FromMilliseconds(180);
        var delay = TimeSpan.FromMilliseconds(growing ? 120 : 60);
        var easeOut = new CubicEase { EasingMode = EasingMode.EaseOut };

        foreach (var view in AllViews)
        {
            bool isTarget = ReferenceEquals(view, target);
            view.IsHitTestVisible = isTarget;

            var opacity = new DoubleAnimation(isTarget ? 1 : 0, isTarget ? fadeIn : fadeOut)
            {
                BeginTime = isTarget ? delay : TimeSpan.Zero,
                EasingFunction = easeOut,
            };
            view.BeginAnimation(OpacityProperty, opacity, HandoffBehavior.SnapshotAndReplace);

            if (isTarget && view.RenderTransform is ScaleTransform scale)
            {
                double startScale = growing ? 0.94 : 1.04;
                var zoom = new DoubleAnimation(startScale, 1, fadeIn + delay)
                {
                    EasingFunction = new SpringEase { DampingRatio = 0.8, Stiffness = 10 },
                };
                scale.BeginAnimation(ScaleTransform.ScaleXProperty, zoom);
                scale.BeginAnimation(ScaleTransform.ScaleYProperty, zoom);
            }
        }
    }

    private static void Animate(IAnimatable target, DependencyProperty property, double to,
        TimeSpan duration, IEasingFunction ease)
    {
        // 不写 From：从当前值（可能正处在上一段动画中途）继续，状态快速切换时也不会跳
        var animation = new DoubleAnimation(to, duration) { EasingFunction = ease };
        target.BeginAnimation(property, animation, HandoffBehavior.SnapshotAndReplace);
    }

    private void ApplyCornerRadius()
    {
        double r = Math.Max(0, CapsuleRadius);
        var corner = new CornerRadius(r);
        Capsule.CornerRadius = corner;
        ShadowLayer.CornerRadius = corner;
        UpdateClip(Capsule.RenderSize);
    }

    private void OnCapsuleSizeChanged(object sender, SizeChangedEventArgs e) => UpdateClip(e.NewSize);

    /// <summary>Border 的圆角不会裁剪子元素，用同样圆角的 Clip 把内容裁进胶囊里。</summary>
    private void UpdateClip(Size size)
    {
        if (size.Width <= 0 || size.Height <= 0) return;
        double r = Math.Min(Math.Max(0, CapsuleRadius), Math.Min(size.Width, size.Height) / 2);
        Capsule.Clip = new RectangleGeometry(new Rect(size), r, r);
    }

    /// <summary>紧凑状态下指示点呼吸闪烁。只在可见时运行，避免空闲时持续占用 CPU。</summary>
    private void UpdateCompactPulse(bool enabled)
    {
        if (!enabled)
        {
            CompactDotScale.BeginAnimation(ScaleTransform.ScaleXProperty, null);
            CompactDotScale.BeginAnimation(ScaleTransform.ScaleYProperty, null);
            return;
        }

        var pulse = new DoubleAnimation(1, 0.7, TimeSpan.FromMilliseconds(900))
        {
            AutoReverse = true,
            RepeatBehavior = RepeatBehavior.Forever,
            EasingFunction = new SineEase { EasingMode = EasingMode.EaseInOut },
        };
        CompactDotScale.BeginAnimation(ScaleTransform.ScaleXProperty, pulse);
        CompactDotScale.BeginAnimation(ScaleTransform.ScaleYProperty, pulse);
    }

    // ───────────────────────────── 对外接口 ─────────────────────────────

    /// <summary>设置常驻的基础状态（以后由计时器等模块调用；音乐会自动处理）。</summary>
    public void SetBaseState(IslandState state, string? compactTitle = null)
    {
        if (state is not (IslandState.Idle or IslandState.Compact))
            throw new ArgumentException("基础状态只能是 Idle 或 Compact", nameof(state));

        if (compactTitle is not null) CompactTitle.Text = compactTitle;
        _baseState = state;
        UpdateState();
    }

    /// <summary>弹出一条临时通知，若干秒后自动缩回。</summary>
    public void ShowNotification(string title, string body, string icon = "🔔", double seconds = 3.5)
    {
        NotificationTitle.Text = title;
        NotificationBody.Text = body;
        NotificationIcon.Text = icon;

        _hasNotification = true;
        _notificationTimer.Stop();
        _notificationTimer.Interval = TimeSpan.FromSeconds(seconds);
        _notificationTimer.Start();

        UpdateState();
    }

    public void DismissNotification()
    {
        _notificationTimer.Stop();
        _hasNotification = false;
        UpdateState();
    }

    // ───────────────────────────── 时钟 ─────────────────────────────

    private void UpdateClock()
    {
        DateTime now = DateTime.Now;
        string time = now.ToString("HH:mm", CultureInfo.InvariantCulture);
        IdleClock.Text = time;
        CompactClock.Text = time;
        ExpandedClock.Text = time;
        ExpandedDate.Text = now.ToString("M月d日 ddd", DateCulture);

        // 对齐到下一分钟再刷新，而不是每秒都跑
        _clockTimer.Stop();
        _clockTimer.Interval = TimeSpan.FromSeconds(60 - now.Second) - TimeSpan.FromMilliseconds(now.Millisecond)
                               + TimeSpan.FromMilliseconds(50);
        _clockTimer.Start();
    }

    // ───────────────────────────── 悬停展开 / 离开收起 ─────────────────────────────

    private void OnIslandMouseEnter(object sender, MouseEventArgs e)
    {
        if (_hiddenForFullscreen) return;
        _hoverOutTimer.Stop();
        if (!_isHovering && !_hoverInTimer.IsEnabled) _hoverInTimer.Start();
    }

    private void OnIslandMouseLeave(object sender, MouseEventArgs e)
    {
        if (_isHovering) _hoverOutTimer.Start();
        else if (!IsCursorOverIsland()) _hoverInTimer.Stop();
    }

    private void Expand(IslandTab? forcedTab = null)
    {
        if (!_isHovering)
        {
            // 从音乐胶囊展开时直接显示播放器，其他情况回到上次用的标签
            IslandTab tab = forcedTab
                            ?? (ReferenceEquals(_currentView, MediaCompactView) ? IslandTab.Music : PreferredTab);
            ShowTab(tab, remember: false);
            _isHovering = true;
            UpdateState();
        }
        else if (forcedTab is { } t)
        {
            ShowTab(t, remember: false);
        }

        _collapseWatchdog.Start();
    }

    /// <summary>鼠标不在岛上、也没有在打字或拖文件时收起。</summary>
    private void TryCollapse()
    {
        if (!_isHovering)
        {
            _collapseWatchdog.Stop();
            return;
        }
        if (ShouldStayExpanded()) return;

        _collapseWatchdog.Stop();
        _hoverOutTimer.Stop();
        _isHovering = false;
        EndTyping(restoreFocus: true);
        UpdateState();
    }

    private bool ShouldStayExpanded() =>
        _holdExpanded || IsCursorOverIsland() || CapsuleMenu.IsOpen || _isDraggingOver || (_isTyping && IsActive);

    /// <summary>直接问系统光标在哪里，比 WPF 的 IsMouseOver 可靠（拖放、快速移动时也准）。</summary>
    private bool IsCursorOverIsland()
    {
        if (PresentationSource.FromVisual(this) is null || !NativeMethods.GetCursorPos(out var pt)) return false;

        Point cursor = PointFromScreen(new Point(pt.X, pt.Y));
        return BoundsOf(CapsuleHost).Contains(cursor) || BoundsOf(HotZone).Contains(cursor);
    }

    private Rect BoundsOf(FrameworkElement element)
    {
        Rect bounds = element.TransformToAncestor(this).TransformBounds(new Rect(element.RenderSize));
        bounds.Inflate(2, 2);
        return bounds;
    }

    private void OnContextMenuClosed(object sender, RoutedEventArgs e)
    {
        if (_isHovering) _hoverOutTimer.Start();
    }

    private void OnCapsuleClick(object sender, MouseButtonEventArgs e)
    {
        // 点击通知直接关掉它
        if (_hasNotification && !_isHovering) DismissNotification();
    }

    // ───────────────────────────── 右键菜单 ─────────────────────────────

    private void OnDemoNotification(object sender, RoutedEventArgs e)
    {
        // 点菜单时鼠标还停在岛上，先强制收起展开态，好让通知立刻可见
        _hoverInTimer.Stop();
        _hoverOutTimer.Stop();
        _collapseWatchdog.Stop();
        _isHovering = false;
        ShowNotification("新消息", "这是一条来自灵动岛的演示通知", "💬");
    }

    private void OnExit(object sender, RoutedEventArgs e) => Application.Current.Shutdown();
}
