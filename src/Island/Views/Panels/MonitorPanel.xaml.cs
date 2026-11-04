using System.Windows.Controls;
using System.Windows.Media;
using System.Windows.Threading;
using Island.Services;

namespace Island.Views.Panels;

/// <summary>系统监控：只在标签可见时每秒采样一次，看不见就停。</summary>
public partial class MonitorPanel : UserControl
{
    private static readonly Brush Normal = Frozen(Color.FromRgb(0x30, 0xD1, 0x58));
    private static readonly Brush Busy = Frozen(Color.FromRgb(0xFF, 0x9F, 0x0A));
    private static readonly Brush Critical = Frozen(Color.FromRgb(0xFF, 0x45, 0x3A));

    private readonly SystemMonitor _monitor = new();
    private readonly DispatcherTimer _timer = new() { Interval = TimeSpan.FromSeconds(1) };

    public MonitorPanel()
    {
        InitializeComponent();
        _timer.Tick += (_, _) => Update();
    }

    public void SetActive(bool active)
    {
        if (active == _timer.IsEnabled) return;
        if (active)
        {
            _monitor.Sample(); // 建立基线，下一秒开始显示差值
            _timer.Start();
        }
        else
        {
            _timer.Stop();
        }
    }

    private void Update()
    {
        SystemSample s = _monitor.Sample();

        CpuText.Text = $"{s.CpuPercent:0}%";
        CpuBar.Value = s.CpuPercent;
        CpuBar.Foreground = s.CpuPercent switch { >= 90 => Critical, >= 70 => Busy, _ => Normal };

        MemText.Text = $"{s.MemoryPercent:0}%";
        MemBar.Value = s.MemoryPercent;
        MemDetail.Text = $"{SystemMonitor.FormatBytes(s.MemoryUsedBytes)} / {SystemMonitor.FormatBytes(s.MemoryTotalBytes)}";

        DownText.Text = SystemMonitor.FormatSpeed(s.DownloadBytesPerSecond);
        UpText.Text = SystemMonitor.FormatSpeed(s.UploadBytesPerSecond);
    }

    private static Brush Frozen(Color color)
    {
        var brush = new SolidColorBrush(color);
        brush.Freeze();
        return brush;
    }
}
