namespace Island.Core;

/// <summary>岛的外观状态。</summary>
public enum IslandState
{
    /// <summary>待机：最小的胶囊，只显示时间。</summary>
    Idle,

    /// <summary>紧凑：稍宽，左图标右信息（音乐、计时器等常驻活动）。</summary>
    Compact,

    /// <summary>通知：临时弹出几秒后自动缩回。</summary>
    Notification,

    /// <summary>展开：鼠标悬停时变成带功能标签的面板。</summary>
    Expanded,
}

/// <summary>每个状态对应的胶囊尺寸（单位：DIP）。</summary>
public readonly record struct IslandMetrics(double Width, double Height, double CornerRadius)
{
    public static IslandMetrics For(IslandState state) => state switch
    {
        IslandState.Idle => new(140, 34, 17),
        IslandState.Compact => new(260, 38, 19),
        IslandState.Notification => new(340, 64, 32),
        IslandState.Expanded => new(440, 260, 32),
        _ => throw new ArgumentOutOfRangeException(nameof(state), state, null),
    };

    /// <summary>窗口要能容纳最大的状态，再留出阴影的空间。</summary>
    public const double WindowWidth = 500;
    public const double WindowHeight = 300;

    /// <summary>胶囊距离屏幕（工作区）顶部的距离。</summary>
    public const double TopMargin = 6;
}
