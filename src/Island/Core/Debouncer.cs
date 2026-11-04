using System.Windows.Threading;

namespace Island.Core;

/// <summary>短时间内多次触发只执行最后一次（用于合并频繁的保存）。</summary>
public sealed class Debouncer
{
    private readonly DispatcherTimer _timer;
    private readonly Action _action;

    public Debouncer(TimeSpan delay, Action action)
    {
        _action = action;
        _timer = new DispatcherTimer { Interval = delay };
        _timer.Tick += (_, _) => Flush();
    }

    public void Trigger()
    {
        _timer.Stop();
        _timer.Start();
    }

    /// <summary>如果有待执行的操作，立即执行（退出前调用）。</summary>
    public void Flush()
    {
        if (!_timer.IsEnabled) return;
        _timer.Stop();
        _action();
    }
}
