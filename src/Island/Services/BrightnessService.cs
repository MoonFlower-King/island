using System.Management;
using System.Windows.Threading;
using Island.Core;

namespace Island.Services;

/// <summary>
/// 屏幕亮度变化（WMI 的 WmiMonitorBrightnessEvent）。
/// 只有笔记本内置屏幕之类支持系统亮度调节的显示器会上报；台式机外接显示器通常没有，这时什么也不做。
/// </summary>
public sealed class BrightnessService : IDisposable
{
    private readonly Dispatcher _dispatcher;
    private ManagementEventWatcher? _watcher;
    private bool _disposed;

    public BrightnessService(Dispatcher dispatcher) => _dispatcher = dispatcher;

    /// <summary>亮度变化，0~1（界面线程）。</summary>
    public event Action<double>? Changed;

    /// <summary>在后台启动监听，避免 WMI 初始化拖慢启动。</summary>
    public void Start() => Task.Run(() =>
    {
        try
        {
            var watcher = new ManagementEventWatcher(
                new ManagementScope(@"root\WMI"),
                new EventQuery("SELECT * FROM WmiMonitorBrightnessEvent"));
            watcher.EventArrived += OnEventArrived;
            watcher.Start();

            if (_disposed)
            {
                watcher.Stop();
                watcher.Dispose();
                return;
            }
            _watcher = watcher;
            Log.Info("亮度监听已启动");
        }
        catch (Exception ex)
        {
            // 台式机、虚拟机通常不支持，属于正常情况
            Log.Info($"此设备不支持亮度监听：{ex.Message}");
        }
    });

    private void OnEventArrived(object sender, EventArrivedEventArgs e)
    {
        try
        {
            if (e.NewEvent.Properties["Brightness"]?.Value is not byte brightness) return;
            double level = Math.Clamp(brightness / 100.0, 0, 1);
            _dispatcher.BeginInvoke(new Action(() =>
            {
                if (!_disposed) Changed?.Invoke(level);
            }));
        }
        catch (Exception ex)
        {
            Log.Error("读取亮度事件失败", ex);
        }
    }

    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;
        try
        {
            _watcher?.Stop();
            _watcher?.Dispose();
        }
        catch
        {
            // 退出时忽略
        }
        _watcher = null;
    }
}
