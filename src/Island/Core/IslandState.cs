namespace Island.Core;

/// <summary>岛的外观状态。</summary>
public enum IslandState
{
    /// <summary>待机：默认是贴在屏幕顶端的一条细线，也可以设置成显示时间的小胶囊。</summary>
    Idle,

    /// <summary>紧凑：稍宽，左图标右信息（音乐、计时器等常驻活动）。</summary>
    Compact,

    /// <summary>通知：临时弹出几秒后自动缩回。</summary>
    Notification,

    /// <summary>音量 / 亮度提示：调节时显示一条进度，停手后很快缩回。</summary>
    Level,

    /// <summary>展开：鼠标悬停时变成带功能标签的面板。</summary>
    Expanded,
}

/// <summary>岛贴在屏幕的哪条边上。</summary>
public enum DockEdge
{
    /// <summary>顶部正中（默认）。</summary>
    Top,

    /// <summary>左侧正中：待机时是一条竖线，展开时从左边滑出。</summary>
    Left,

    /// <summary>右侧正中。</summary>
    Right,
}

/// <summary>
/// 胶囊在某个状态下的形状（单位：DIP）。<see cref="Top"/> 是距离所贴屏幕边缘的距离
/// （顶部模式下是离窗口顶端，左右模式下是离窗口左 / 右边），
/// 可以是负数：细线状态把一半藏到屏幕外，只露出带圆角的另一半。
/// </summary>
public readonly record struct IslandMetrics(double Width, double Height, double CornerRadius, double Top)
{
    /// <summary>胶囊离屏幕（工作区）边缘的距离。</summary>
    public const double TopMargin = 6;

    /// <summary>待机细线露出来的厚度。</summary>
    public const double LineVisibleHeight = 5;

    public static IslandMetrics For(IslandState state, bool idleAsLine = true, DockEdge edge = DockEdge.Top) => state switch
    {
        IslandState.Idle when idleAsLine => edge == DockEdge.Top ? Line : VerticalLine,
        IslandState.Idle => new(140, 34, 17, TopMargin),
        IslandState.Compact => new(260, 38, 19, TopMargin),
        IslandState.Notification => new(340, 64, 32, TopMargin),
        IslandState.Level => new(300, 40, 20, TopMargin),
        IslandState.Expanded => new(440, 260, 32, TopMargin),
        _ => throw new ArgumentOutOfRangeException(nameof(state), state, null),
    };

    /// <summary>待机细线：高 10、上移 5，只露出下半截 5 像素和圆角。</summary>
    public static IslandMetrics Line { get; } = new(120, LineVisibleHeight * 2, LineVisibleHeight, -LineVisibleHeight);

    /// <summary>贴在左右两侧时的待机细线：竖着的。</summary>
    public static IslandMetrics VerticalLine { get; } = new(LineVisibleHeight * 2, 120, LineVisibleHeight, -LineVisibleHeight);

    /// <summary>离所贴边缘最远的一侧（顶部模式下是底边）。</summary>
    public double Bottom => Top + Height;

    /// <summary>
    /// 胶囊在宿主区域（和窗口一样大）里的矩形。顶部模式水平居中、贴着上边；
    /// 左右模式垂直居中、贴着左边或右边。
    /// </summary>
    public IslandRect RectIn(DockEdge edge, double hostWidth = WindowWidth, double hostHeight = WindowHeight) => edge switch
    {
        DockEdge.Left => new IslandRect(Top, (hostHeight - Height) / 2, Width, Height),
        DockEdge.Right => new IslandRect(hostWidth - Width - Top, (hostHeight - Height) / 2, Width, Height),
        _ => new IslandRect((hostWidth - Width) / 2, Top, Width, Height),
    };

    /// <summary>窗口要能容纳最大的状态，再留出阴影的空间。胶囊所在区域（CapsuleHost）和窗口一样大。</summary>
    public const double WindowWidth = 500;
    public const double WindowHeight = 300;
}

/// <summary>和 WPF 无关的矩形，方便在核心逻辑和测试里用。</summary>
public readonly record struct IslandRect(double X, double Y, double Width, double Height)
{
    public double Right => X + Width;
    public double Bottom => Y + Height;
}
