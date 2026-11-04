using System.Collections.ObjectModel;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Text.Json.Serialization;
using System.Windows;
using Island.Core;

namespace Island.Services;

public sealed class ClipboardEntry
{
    public string Text { get; set; } = "";
    public DateTime CopiedAt { get; set; } = DateTime.Now;

    /// <summary>列表里显示的单行预览。</summary>
    [JsonIgnore]
    public string Preview
    {
        get
        {
            string oneLine = string.Join(" ", Text.Split(['\r', '\n', '\t'], StringSplitOptions.RemoveEmptyEntries)).Trim();
            return oneLine.Length > 120 ? oneLine[..120] + "…" : oneLine;
        }
    }

    [JsonIgnore]
    public string TimeLabel => CopiedAt.Date == DateTime.Today ? CopiedAt.ToString("HH:mm") : CopiedAt.ToString("M/d HH:mm");
}

/// <summary>
/// 剪贴板历史：窗口收到 WM_CLIPBOARDUPDATE 时调用 <see cref="OnClipboardUpdated"/>。
/// 只记录文本；密码管理器等标记了“不要记录”的内容会被跳过。
/// </summary>
public sealed class ClipboardService
{
    private const string FileName = "clipboard.json";
    private const int MaxEntries = 50;
    private const int MaxLength = 20_000;

    // 密码管理器复制密码时会附带这些格式，表示“别记录我”
    private static readonly string[] ExcludeFormats =
    [
        "ExcludeClipboardContentFromMonitorProcessing",
        "Clipboard Viewer Ignore",
    ];

    private readonly AppSettings _settings;
    private readonly Debouncer _save;
    private string? _ignoreNext;

    public ObservableCollection<ClipboardEntry> Items { get; }

    public ClipboardService(AppSettings settings)
    {
        _settings = settings;
        var saved = settings.PersistClipboardHistory ? JsonStore.Load<List<ClipboardEntry>>(FileName) : [];
        Items = new ObservableCollection<ClipboardEntry>(saved.Take(MaxEntries));
        _save = new Debouncer(TimeSpan.FromSeconds(1), Save);
    }

    public async void OnClipboardUpdated()
    {
        string? text;
        try
        {
            text = await TryReadTextAsync();
        }
        catch (Exception ex)
        {
            Log.Error("读取剪贴板失败", ex);
            return;
        }
        if (string.IsNullOrWhiteSpace(text) || text.Length > MaxLength) return;

        // 自己“复制回剪贴板”触发的更新不重复记录
        if (_ignoreNext is not null && text == _ignoreNext)
        {
            _ignoreNext = null;
            return;
        }

        var existing = Items.FirstOrDefault(i => i.Text == text);
        if (existing is not null) Items.Remove(existing);

        Items.Insert(0, new ClipboardEntry { Text = text });
        while (Items.Count > MaxEntries) Items.RemoveAt(Items.Count - 1);
        _save.Trigger();
    }

    /// <summary>把历史中的一条重新放回剪贴板，并移到最前面。</summary>
    public bool CopyBack(ClipboardEntry entry)
    {
        _ignoreNext = entry.Text;
        if (!Retry(() => Clipboard.SetText(entry.Text)))
        {
            _ignoreNext = null;
            return false;
        }

        int index = Items.IndexOf(entry);
        if (index > 0) Items.Move(index, 0);
        entry.CopiedAt = DateTime.Now;
        _save.Trigger();
        return true;
    }

    public void Remove(ClipboardEntry entry)
    {
        Items.Remove(entry);
        _save.Trigger();
    }

    public void Clear()
    {
        Items.Clear();
        _save.Trigger();
    }

    public void Flush() => _save.Flush();

    /// <summary>立即写盘（重新开启“保存历史”时调用）。</summary>
    public void SaveNow() => Save();

    private void Save()
    {
        if (_settings.PersistClipboardHistory) JsonStore.Save(FileName, Items.ToList());
    }

    /// <summary>读取文字；剪贴板被占用时异步等待重试，不阻塞界面线程。</summary>
    private static async Task<string?> TryReadTextAsync()
    {
        for (int attempt = 0; attempt < 5; attempt++)
        {
            try
            {
                IDataObject? data = Clipboard.GetDataObject();
                if (data is null) return null;
                foreach (string format in ExcludeFormats)
                    if (data.GetDataPresent(format)) return null;
                return data.GetDataPresent(DataFormats.UnicodeText)
                    ? data.GetData(DataFormats.UnicodeText) as string
                    : null;
            }
            catch (ExternalException ex)
            {
                Debug.WriteLine($"[Clipboard] 第 {attempt + 1} 次读取失败：{ex.Message}");
                await Task.Delay(40);
            }
        }
        return null;
    }

    /// <summary>剪贴板可能正被别的程序占用（CLIPBRD_E_CANT_OPEN），稍等重试几次。</summary>
    private static bool Retry(Action action)
    {
        for (int attempt = 0; attempt < 5; attempt++)
        {
            try
            {
                action();
                return true;
            }
            catch (COMException ex)
            {
                Debug.WriteLine($"[Clipboard] 第 {attempt + 1} 次访问失败：{ex.Message}");
                Thread.Sleep(30);
            }
            catch (ExternalException ex)
            {
                Debug.WriteLine($"[Clipboard] 访问失败：{ex.Message}");
                Thread.Sleep(30);
            }
        }
        return false;
    }
}
