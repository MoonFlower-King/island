using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;
using System.Windows.Threading;
using Island.Services;

namespace Island.Views.Panels;

public partial class ClipboardPanel : UserControl
{
    private ClipboardService? _service;

    private const string DefaultHint = "点击任意一条即可重新复制";
    private readonly DispatcherTimer _hintTimer = new() { Interval = TimeSpan.FromSeconds(1.5) };

    public ClipboardPanel()
    {
        InitializeComponent();
        _hintTimer.Tick += (_, _) =>
        {
            _hintTimer.Stop();
            Hint.Text = DefaultHint;
        };
    }

    public void Attach(ClipboardService service)
    {
        _service = service;
        List.ItemsSource = service.Items;
        service.Items.CollectionChanged += (_, _) => Refresh();
        Refresh();
    }

    private void Refresh()
    {
        if (_service is null) return;
        Empty.Visibility = _service.Items.Count == 0 ? Visibility.Visible : Visibility.Collapsed;
    }

    private void OnEntryClick(object sender, MouseButtonEventArgs e)
    {
        if (_service is null || sender is not FrameworkElement { DataContext: ClipboardEntry entry }) return;
        ShowHint(_service.CopyBack(entry) ? "已复制到剪贴板 ✓" : "剪贴板被占用，请重试");
    }

    private void ShowHint(string text)
    {
        Hint.Text = text;
        _hintTimer.Stop();
        _hintTimer.Start();
    }

    private void OnDelete(object sender, RoutedEventArgs e)
    {
        if (sender is FrameworkElement { DataContext: ClipboardEntry entry }) _service?.Remove(entry);
    }

    private void OnClear(object sender, RoutedEventArgs e) => _service?.Clear();
}
