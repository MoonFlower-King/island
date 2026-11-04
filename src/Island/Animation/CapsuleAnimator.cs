using System.Windows;
using System.Windows.Media;
using Island.Core;

namespace Island.Animation;

/// <summary>弹簧参数：response 是大致的完成时间（秒），damping 是阻尼比（1 为不回弹）。</summary>
public readonly record struct SpringParameters(double Response, double Damping)
{
    /// <summary>展开：有一点回弹，像 iPhone 的灵动岛。</summary>
    public static SpringParameters Expand { get; } = new(0.42, 0.74);

    /// <summary>收起：快且几乎不回弹，离开后立刻让出屏幕。</summary>
    public static SpringParameters Collapse { get; } = new(0.30, 0.92);

    public double Stiffness => Math.Pow(2 * Math.PI / Response, 2);
    public double Friction => 4 * Math.PI * Damping / Response;
}

/// <summary>
/// 一维阻尼弹簧。和固定时长的缓动动画不同，弹簧保存了速度：
/// 动画进行到一半换了目标（比如刚展开就移开鼠标），会从当前速度自然转向，不会顿一下。
/// </summary>
public sealed class Spring
{
    private const double SubStep = 1.0 / 480;

    public Spring(double value)
    {
        Value = value;
        Target = value;
    }

    public double Value { get; private set; }
    public double Velocity { get; private set; }
    public double Target { get; set; }

    public bool IsSettled(double tolerance) =>
        Math.Abs(Value - Target) < tolerance && Math.Abs(Velocity) < tolerance * 10;

    public void Step(double seconds, SpringParameters p)
    {
        double k = p.Stiffness, c = p.Friction;
        // 固定小步长积分，帧率高低（60/120/144Hz）都得到同样的运动曲线
        while (seconds > 0)
        {
            double dt = Math.Min(SubStep, seconds);
            double acceleration = -k * (Value - Target) - c * Velocity;
            Velocity += acceleration * dt;
            Value += Velocity * dt;
            seconds -= dt;
        }
    }

    public void Snap()
    {
        Value = Target;
        Velocity = 0;
    }

    public void JumpTo(double value)
    {
        Value = Target = value;
        Velocity = 0;
    }
}

/// <summary>
/// 驱动胶囊形状：每一帧（CompositionTarget.Rendering）推进四根弹簧——宽、高、顶部位置、圆角——
/// 再写进共享的 RectangleGeometry。只在运动时订阅渲染事件，停下后不占任何 CPU。
/// </summary>
public sealed class CapsuleAnimator
{
    private const double Tolerance = 0.05;

    private readonly RectangleGeometry _geometry;
    private readonly double _hostWidth;
    private readonly Spring _width, _height, _top, _radius;
    private SpringParameters _parameters = SpringParameters.Expand;
    private TimeSpan _lastFrame;
    private bool _running;

    public CapsuleAnimator(RectangleGeometry geometry, double hostWidth, IslandMetrics initial)
    {
        _geometry = geometry;
        _hostWidth = hostWidth;
        _width = new Spring(initial.Width);
        _height = new Spring(initial.Height);
        _top = new Spring(initial.Top);
        _radius = new Spring(initial.CornerRadius);
        Apply();
    }

    /// <summary>当前（动画中的）胶囊矩形，相对于 CapsuleHost。</summary>
    public Rect CurrentRect => _geometry.Rect;

    public bool IsAnimating => _running;

    /// <summary>每帧形状更新之后触发（用于同步阴影等）。</summary>
    public event Action? Updated;

    public void AnimateTo(IslandMetrics target, SpringParameters parameters)
    {
        _parameters = parameters;
        _width.Target = target.Width;
        _height.Target = target.Height;
        _top.Target = target.Top;
        _radius.Target = target.CornerRadius;

        if (_running) return;
        _running = true;
        _lastFrame = TimeSpan.Zero;
        CompositionTarget.Rendering += OnRendering;
    }

    public void JumpTo(IslandMetrics target)
    {
        _width.JumpTo(target.Width);
        _height.JumpTo(target.Height);
        _top.JumpTo(target.Top);
        _radius.JumpTo(target.CornerRadius);
        Stop();
        Apply();
    }

    private void OnRendering(object? sender, EventArgs e)
    {
        TimeSpan now = e is RenderingEventArgs r ? r.RenderingTime : TimeSpan.Zero;
        if (_lastFrame == TimeSpan.Zero || now <= _lastFrame)
        {
            // 第一帧只记录时间；同一帧可能触发多次 Rendering，跳过重复的
            if (_lastFrame == TimeSpan.Zero) _lastFrame = now;
            return;
        }

        // 窗口被拖慢（比如系统卡顿）时限制单帧步长，避免一下子跳过整段动画
        double seconds = Math.Min((now - _lastFrame).TotalSeconds, 1.0 / 20);
        _lastFrame = now;

        Spring[] springs = [_width, _height, _top, _radius];
        foreach (Spring spring in springs) spring.Step(seconds, _parameters);

        if (springs.All(s => s.IsSettled(Tolerance)))
        {
            foreach (Spring spring in springs) spring.Snap();
            Stop();
        }

        Apply();
    }

    private void Stop()
    {
        if (!_running) return;
        _running = false;
        CompositionTarget.Rendering -= OnRendering;
    }

    private void Apply()
    {
        double w = Math.Max(1, _width.Value);
        double h = Math.Max(1, _height.Value);
        double r = Math.Clamp(_radius.Value, 0, Math.Min(w, h) / 2);

        _geometry.Rect = new Rect((_hostWidth - w) / 2, _top.Value, w, h);
        _geometry.RadiusX = r;
        _geometry.RadiusY = r;
        Updated?.Invoke();
    }
}
