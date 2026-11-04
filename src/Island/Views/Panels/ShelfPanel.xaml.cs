using System.Diagnostics;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;
using Island.Services;

namespace Island.Views.Panels;

public partial class ShelfPanel : UserControl
{
    private ShelfService? _service;
    private Point _dragStart;
    private ShelfItem? _pressed;

    public ShelfPanel() => InitializeComponent();

    public void Attach(ShelfService service)
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

    private void OnItemMouseDown(object sender, MouseButtonEventArgs e)
    {
        if (sender is not FrameworkElement { DataContext: ShelfItem item }) return;

        if (e.ClickCount == 2)
        {
            Open(item.Path);
            e.Handled = true;
            return;
        }

        _pressed = item;
        _dragStart = e.GetPosition(this);
    }

    /// <summary>按住拖动超过系统阈值后，把文件拖出岛（相当于从资源管理器里拖文件）。</summary>
    private void OnItemMouseMove(object sender, MouseEventArgs e)
    {
        if (_pressed is null || e.LeftButton != MouseButtonState.Pressed) return;

        Vector moved = e.GetPosition(this) - _dragStart;
        if (Math.Abs(moved.X) < SystemParameters.MinimumHorizontalDragDistance &&
            Math.Abs(moved.Y) < SystemParameters.MinimumVerticalDragDistance) return;

        ShelfItem item = _pressed;
        _pressed = null;
        if (!item.Exists) return;

        var data = new DataObject(DataFormats.FileDrop, new[] { item.Path });
        DragDrop.DoDragDrop((DependencyObject)sender, data, DragDropEffects.Copy | DragDropEffects.Move);

        // 被“移动”到别处后原路径就不存在了，顺手移出中转站
        if (!item.Exists) _service?.Remove(item);
    }

    private void OnReveal(object sender, RoutedEventArgs e)
    {
        if (sender is FrameworkElement { DataContext: ShelfItem item } && item.Exists)
            Start("explorer.exe", $"/select,\"{item.Path}\"");
    }

    private void OnRemove(object sender, RoutedEventArgs e)
    {
        if (sender is FrameworkElement { DataContext: ShelfItem item }) _service?.Remove(item);
    }

    private void OnClear(object sender, RoutedEventArgs e) => _service?.Clear();

    private static void Open(string path) => Start(path, null);

    private static void Start(string fileName, string? arguments)
    {
        try
        {
            Process.Start(new ProcessStartInfo(fileName) { Arguments = arguments ?? "", UseShellExecute = true });
        }
        catch (Exception ex)
        {
            Debug.WriteLine($"[Shelf] 打开失败：{ex.Message}");
        }
    }
}
