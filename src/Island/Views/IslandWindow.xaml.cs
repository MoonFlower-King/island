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
/// 显示哪个状态由三件事决定（优先级从高到低）：
///   1. 鼠标悬停 → 展开
///   2. 有临时通知 → 通知
///   3. 否则 → 基础状态（有音乐在播放或手动设置时为紧凑，否则待机）
///
/// 同一个状态可能对应不同的内容视图（例如展开态有音乐时显示播放器），见 <see cref="ViewFor"/>。
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

    private readonly DispatcherTimer _hoverInTimer = new() { Interval = TimeSpan.FromMilliseconds(120) };
    private readonly DispatcherTimer _hoverOutTimer = new() { Interval = TimeSpan.FromMilliseconds(320) };
    private readonly DispatcherTimer _notificationTimer = new();
    private readonly DispatcherTimer _clockTimer = new();
    private readonly DispatcherTimer _topmostTimer = new() { Interval = TimeSpan.FromSeconds(3) };

    private IntPtr _hwnd;
    private IslandState _baseState = IslandState.Idle;
    private IslandState _currentState = IslandState.Idle;
    private FrameworkElement? _currentView;
    private bool _isHovering;
    private bool _hasNotification;

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
            _isHovering = true;
            UpdateState();
        };
        _hoverOutTimer.Tick += (_, _) =>
        {
            _hoverOutTimer.Stop();
            _isHovering = false;
            UpdateState();
        };
        _notificationTimer.Tick += (_, _) => DismissNotification();
        _clockTimer.Tick += (_, _) => UpdateClock();
        _topmostTimer.Tick += (_, _) =>
        {
            if (_hwnd != IntPtr.Zero) NativeMethods.BringToTopmost(_hwnd);
        };

        SystemEvents.DisplaySettingsChanged += OnDisplaySettingsChanged;
        SystemParameters.StaticPropertyChanged += OnSystemParametersChanged;
        Closed += OnClosed;

        _currentView = IdleView;
        InitializeMedia();

        PositionWindow();
        UpdateClock();
    }

    private FrameworkElement[] AllViews =>
        [IdleView, CompactView, MediaCompactView, NotificationView, ExpandedView, MediaExpandedView];

    // ───────────────────────────── 窗口与 Win32 ─────────────────────────────

    protected override void OnSourceInitialized(EventArgs e)
    {
        base.OnSourceInitialized(e);

        _hwnd = new WindowInteropHelper(this).Handle;
        NativeMethods.MakeOverlay(_hwnd);
        HwndSource.FromHwnd(_hwnd)?.AddHook(WndProc);

        _topmostTimer.Start();
    }

    private static IntPtr WndProc(IntPtr hwnd, int msg, IntPtr wParam, IntPtr lParam, ref bool handled)
    {
        // 点击岛时不要激活窗口，否则用户正在打字的窗口会失去焦点
        if (msg == NativeMethods.WM_MOUSEACTIVATE)
        {
            handled = true;
            return new IntPtr(NativeMethods.MA_NOACTIVATE);
        }
        return IntPtr.Zero;
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

        foreach (var timer in new[] { _hoverInTimer, _hoverOutTimer, _notificationTimer, _clockTimer, _topmostTimer })
            timer.Stop();

        ShutdownMedia();
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
            CrossFadeTo(view, growing: false); // 尺寸不变，只换内容（例如展开时开始放歌）
        else
            TransitionTo(previous, target, view);

        OnActiveViewChanged();
    }

    private void TransitionTo(IslandState from, IslandState to, FrameworkElement view)
    {
        IslandMetrics a = IslandMetrics.For(from);
        IslandMetrics b = IslandMetrics.For(to);
        bool growing = b.Width * b.Height > a.Width * a.Height;

        // 变大时更有弹性，缩小时干脆一些
        var ease = growing
            ? new SpringEase { DampingRatio = 0.66, Stiffness = 11 }
            : new SpringEase { DampingRatio = 0.86, Stiffness = 12 };
        var duration = TimeSpan.FromMilliseconds(growing ? 560 : 440);

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
        IslandState.Expanded => HasMedia ? MediaExpandedView : ExpandedView,
        _ => IdleView,
    };

    /// <summary>可见内容变了：只让当前可见的视图跑动画和定时器，其余全部停掉。</summary>
    private void OnActiveViewChanged()
    {
        UpdateCompactPulse(ReferenceEquals(_currentView, CompactView));
        RefreshMediaAnimations();
    }

    /// <summary>旧内容快速淡出，新内容等胶囊长出一点后再淡入并轻微放大。</summary>
    private void CrossFadeTo(FrameworkElement target, bool growing)
    {
        var fadeOut = TimeSpan.FromMilliseconds(90);
        var fadeIn = TimeSpan.FromMilliseconds(200);
        var delay = TimeSpan.FromMilliseconds(growing ? 140 : 80);
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

    /// <summary>设置常驻的基础状态（以后由音乐、计时器等模块调用）。</summary>
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
        MediaExpandedClock.Text = time;
        ExpandedDate.Text = now.ToString("M月d日 dddd", DateCulture);

        // 对齐到下一分钟再刷新，而不是每秒都跑
        _clockTimer.Stop();
        _clockTimer.Interval = TimeSpan.FromSeconds(60 - now.Second) - TimeSpan.FromMilliseconds(now.Millisecond)
                               + TimeSpan.FromMilliseconds(50);
        _clockTimer.Start();
    }

    // ───────────────────────────── 鼠标交互 ─────────────────────────────

    private void OnCapsuleMouseEnter(object sender, MouseEventArgs e)
    {
        _hoverOutTimer.Stop();
        if (!_isHovering) _hoverInTimer.Start();
    }

    private void OnCapsuleMouseLeave(object sender, MouseEventArgs e)
    {
        _hoverInTimer.Stop();
        if (CapsuleMenu.IsOpen) return; // 右键菜单打开时先不收起
        if (_isHovering) _hoverOutTimer.Start();
    }

    private void OnContextMenuClosed(object sender, RoutedEventArgs e)
    {
        if (!CapsuleHost.IsMouseOver && _isHovering) _hoverOutTimer.Start();
    }

    private void OnCapsuleClick(object sender, MouseButtonEventArgs e)
    {
        // 点击通知直接关掉它
        if (_hasNotification) DismissNotification();
    }

    // ───────────────────────────── 右键菜单（演示用） ─────────────────────────────

    private void OnDemoToggleCompact(object sender, RoutedEventArgs e)
    {
        ReleaseHover();
        SetBaseState(_baseState == IslandState.Compact ? IslandState.Idle : IslandState.Compact,
            "专注计时进行中");
    }

    private void OnDemoNotification(object sender, RoutedEventArgs e)
    {
        ReleaseHover();
        ShowNotification("新消息", "这是一条来自灵动岛的演示通知", "💬");
    }

    /// <summary>点菜单时鼠标还停在岛上，先强制收起展开态，好让演示效果立刻可见。</summary>
    private void ReleaseHover()
    {
        _hoverInTimer.Stop();
        _hoverOutTimer.Stop();
        _isHovering = false;
    }

    private void OnExit(object sender, RoutedEventArgs e) => Application.Current.Shutdown();
}
