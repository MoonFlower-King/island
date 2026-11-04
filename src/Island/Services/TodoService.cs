using System.Collections.ObjectModel;
using System.Collections.Specialized;
using System.ComponentModel;
using System.Windows.Threading;
using Island.Core;

namespace Island.Services;

public sealed class TodoItem : ObservableObject
{
    private string _text = "";
    private bool _done;

    public Guid Id { get; set; } = Guid.NewGuid();
    public DateTime CreatedAt { get; set; } = DateTime.Now;

    public string Text { get => _text; set => Set(ref _text, value); }
    public bool Done { get => _done; set => Set(ref _done, value); }
}

/// <summary>日常待办：增删改后自动保存到 todos.json。</summary>
public sealed class TodoService
{
    private const string FileName = "todos.json";
    private readonly Debouncer _save;
    private readonly Dispatcher _dispatcher = Dispatcher.CurrentDispatcher;

    public ObservableCollection<TodoItem> Items { get; }

    /// <summary>任意待办增删或勾选状态变化。</summary>
    public event Action? Changed;

    public TodoService()
    {
        Items = new ObservableCollection<TodoItem>(JsonStore.Load<List<TodoItem>>(FileName));
        _save = new Debouncer(TimeSpan.FromMilliseconds(400), () => JsonStore.Save(FileName, Items.ToList()));

        foreach (var item in Items) item.PropertyChanged += OnItemChanged;
        Items.CollectionChanged += OnCollectionChanged;
    }

    public int PendingCount => Items.Count(i => !i.Done);

    public void Add(string text)
    {
        text = text.Trim();
        if (text.Length == 0) return;
        // 新待办放在未完成项的最前面
        Items.Insert(0, new TodoItem { Text = text });
    }

    public void Remove(TodoItem item) => Items.Remove(item);

    public void ClearCompleted()
    {
        foreach (var done in Items.Where(i => i.Done).ToList()) Items.Remove(done);
    }

    /// <summary>勾选完成后沉到列表底部，取消勾选回到顶部。</summary>
    private void OnItemChanged(object? sender, PropertyChangedEventArgs e)
    {
        if (sender is TodoItem item && e.PropertyName == nameof(TodoItem.Done))
        {
            // 等勾选框的点击处理完再移动，避免在绑定更新中途改动列表
            _dispatcher.BeginInvoke(new Action(() =>
            {
                int from = Items.IndexOf(item);
                int to = item.Done ? Items.Count - 1 : 0;
                if (from >= 0 && from != to) Items.Move(from, to);
            }));
        }
        _save.Trigger();
        Changed?.Invoke();
    }

    private void OnCollectionChanged(object? sender, NotifyCollectionChangedEventArgs e)
    {
        // Move 只是换位置，订阅关系不变
        if (e.Action != NotifyCollectionChangedAction.Move)
        {
            if (e.OldItems is not null)
                foreach (TodoItem item in e.OldItems) item.PropertyChanged -= OnItemChanged;
            if (e.NewItems is not null)
                foreach (TodoItem item in e.NewItems) item.PropertyChanged += OnItemChanged;
        }
        _save.Trigger();
        Changed?.Invoke();
    }

    public void Flush() => _save.Flush();
}
