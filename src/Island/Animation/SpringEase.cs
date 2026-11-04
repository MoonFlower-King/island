using System.Windows;
using System.Windows.Media.Animation;

namespace Island.Animation;

/// <summary>
/// 阻尼弹簧缓动：模拟灵动岛那种“冲过头一点再回弹”的手感。
///
/// 把弹簧的位移方程归一化到 t ∈ [0, 1]：
///   x(t) = 1 - e^(-ζ·ω·t) · (cos(ωd·t) + (ζ·ω / ωd) · sin(ωd·t))
/// 其中 ωd = ω·√(1-ζ²)，ζ 为阻尼比 (0 &lt; ζ &lt; 1 时会回弹)。
/// </summary>
public sealed class SpringEase : EasingFunctionBase
{
    public static readonly DependencyProperty DampingRatioProperty =
        DependencyProperty.Register(nameof(DampingRatio), typeof(double), typeof(SpringEase),
            new PropertyMetadata(0.72));

    public static readonly DependencyProperty StiffnessProperty =
        DependencyProperty.Register(nameof(Stiffness), typeof(double), typeof(SpringEase),
            new PropertyMetadata(12.0));

    /// <summary>阻尼比。越小回弹越明显，1 表示不回弹。</summary>
    public double DampingRatio
    {
        get => (double)GetValue(DampingRatioProperty);
        set => SetValue(DampingRatioProperty, value);
    }

    /// <summary>无阻尼角频率（在归一化时间内）。越大动作越“利落”。</summary>
    public double Stiffness
    {
        get => (double)GetValue(StiffnessProperty);
        set => SetValue(StiffnessProperty, value);
    }

    public SpringEase()
    {
        // 弹簧本身就是 ease-out 形状，直接用 EaseIn 模式表示“原样输出”
        EasingMode = EasingMode.EaseIn;
    }

    protected override double EaseInCore(double t)
    {
        if (t <= 0) return 0;
        if (t >= 1) return 1;

        double zeta = Math.Clamp(DampingRatio, 0.05, 0.999);
        double omega = Stiffness;
        double omegaD = omega * Math.Sqrt(1 - zeta * zeta);

        double decay = Math.Exp(-zeta * omega * t);
        double value = 1 - decay * (Math.Cos(omegaD * t) + zeta * omega / omegaD * Math.Sin(omegaD * t));

        // 在 t=1 处强制收敛，避免动画结束时“跳”一下
        double end = 1 - Math.Exp(-zeta * omega) *
            (Math.Cos(omegaD) + zeta * omega / omegaD * Math.Sin(omegaD));
        return value + (1 - end) * t;
    }

    protected override Freezable CreateInstanceCore() => new SpringEase();
}
