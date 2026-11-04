using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;
using System.Windows.Threading;
using Island.Services;

namespace Island.Views.Panels;

public partial class TodoPanel : UserControl
{
    private TodoService? _service;

    public TodoPanel() => InitializeComponent();

    /// <summary>待办数量变化（用于在标签上显示角标）。</summary>
    public event Action<int>? PendingCountChanged;

    public void Attach(TodoService service)
    {
        _service = service;
        List.ItemsSource = service.Items;
        service.Changed += Refresh;
        Refresh();
    }

    /// <summary>点开待办标签时把光标放进输入框，直接就能打字。</summary>
    public void FocusInput() =>
        Dispatcher.BeginInvoke(DispatcherPriority.Input, new Action(() => Input.Focus()));

    private void Refresh()
    {
        if (_service is null) return;
        Empty.Visibility = _service.Items.Count == 0 ? Visibility.Visible : Visibility.Collapsed;
        PendingCountChanged?.Invoke(_service.PendingCount);
    }

    private void OnInputKeyDown(object sender, KeyEventArgs e)
    {
        if (e.Key != Key.Enter || _service is null) return;
        _service.Add(Input.Text);
        Input.Clear();
        e.Handled = true;
    }

    private void OnDelete(object sender, RoutedEventArgs e)
    {
        if (sender is FrameworkElement { DataContext: TodoItem item }) _service?.Remove(item);
    }

    private void OnClearCompleted(object sender, RoutedEventArgs e) => _service?.ClearCompleted();
}
