using System.Collections.ObjectModel;
using System.IO;
using System.Text.Json.Serialization;
using Island.Core;

namespace Island.Services;

public sealed class ShelfItem
{
    private string _path = "";
    private (bool IsFolder, string Detail)? _info;

    public string Path
    {
        get => _path;
        set
        {
            _path = value;
            _info = null;
        }
    }

    [JsonIgnore]
    public string Name => System.IO.Path.GetFileName(Path.TrimEnd('\\', '/')) is { Length: > 0 } n ? n : Path;

    /// <summary>每次调用都会查询磁盘（用于判断文件是否还在），不要在绑定里用。</summary>
    [JsonIgnore]
    public bool Exists => File.Exists(Path) || Directory.Exists(Path);

    [JsonIgnore]
    public bool IsFolder => Info.IsFolder;

    /// <summary>按类型挑一个图标（Segoe Fluent Icons 字形）。</summary>
    [JsonIgnore]
    public string Glyph
    {
        get
        {
            if (IsFolder) return "\uE8B7";
            return System.IO.Path.GetExtension(Path).ToLowerInvariant() switch
            {
                ".png" or ".jpg" or ".jpeg" or ".gif" or ".bmp" or ".webp" or ".heic" => "\uEB9F",
                ".mp4" or ".mov" or ".mkv" or ".avi" or ".webm" => "\uE714",
                ".mp3" or ".flac" or ".wav" or ".m4a" or ".aac" or ".ogg" => "\uE8D6",
                ".zip" or ".rar" or ".7z" or ".tar" or ".gz" => "\uF012",
                ".pdf" or ".doc" or ".docx" or ".txt" or ".md" or ".ppt" or ".pptx" or ".xls" or ".xlsx" => "\uE8A5",
                ".exe" or ".msi" => "\uE756",
                _ => "\uE7C3",
            };
        }
    }

    [JsonIgnore]
    public string Detail => Info.Detail;

    /// <summary>
    /// 文件夹/大小只读一次磁盘并缓存。界面绑定会反复读取这些属性，
    /// 每次都访问磁盘的话，遇到网络盘或休眠的移动硬盘会卡住界面。
    /// </summary>
    private (bool IsFolder, string Detail) Info => _info ??= ReadInfo(Path);

    private static (bool, string) ReadInfo(string path)
    {
        try
        {
            if (Directory.Exists(path)) return (true, "文件夹");
            var info = new FileInfo(path);
            return (false, info.Exists ? FormatSize(info.Length) : "文件已不存在");
        }
        catch
        {
            return (false, "");
        }
    }

    private static string FormatSize(long bytes) => bytes switch
    {
        < 1024 => $"{bytes} B",
        < 1024 * 1024 => $"{bytes / 1024.0:0.#} KB",
        < 1024L * 1024 * 1024 => $"{bytes / 1024.0 / 1024:0.#} MB",
        _ => $"{bytes / 1024.0 / 1024 / 1024:0.##} GB",
    };
}

/// <summary>
/// 文件中转站：临时“放”一些文件在岛上，之后再拖到别处。
/// 只记录路径，不复制文件本身，所以不占额外磁盘空间。
/// </summary>
public sealed class ShelfService
{
    private const string FileName = "shelf.json";
    private const int MaxItems = 30;

    public ObservableCollection<ShelfItem> Items { get; }

    public ShelfService()
    {
        // 启动时去掉已被删除或移走的文件
        var saved = JsonStore.Load<List<ShelfItem>>(FileName).Where(i => i.Exists).Take(MaxItems);
        Items = new ObservableCollection<ShelfItem>(saved);
    }

    public void Add(IEnumerable<string> paths)
    {
        foreach (string path in paths)
        {
            if (string.IsNullOrWhiteSpace(path)) continue;
            if (Items.Any(i => string.Equals(i.Path, path, StringComparison.OrdinalIgnoreCase))) continue;
            Items.Insert(0, new ShelfItem { Path = path });
        }
        while (Items.Count > MaxItems) Items.RemoveAt(Items.Count - 1);
        Save();
    }

    public void Remove(ShelfItem item)
    {
        Items.Remove(item);
        Save();
    }

    public void Clear()
    {
        Items.Clear();
        Save();
    }

    private void Save() => JsonStore.Save(FileName, Items.ToList());
}
