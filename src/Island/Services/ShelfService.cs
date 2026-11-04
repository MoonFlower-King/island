using System.Collections.ObjectModel;
using System.IO;
using Island.Core;

namespace Island.Services;

public sealed class ShelfItem
{
    public string Path { get; set; } = "";

    public string Name => System.IO.Path.GetFileName(Path.TrimEnd('\\', '/')) is { Length: > 0 } n ? n : Path;

    public bool IsFolder => Directory.Exists(Path);

    public bool Exists => File.Exists(Path) || Directory.Exists(Path);

    /// <summary>按类型挑一个图标（Segoe Fluent Icons 字形）。</summary>
    public string Glyph
    {
        get
        {
            if (IsFolder) return "";
            return System.IO.Path.GetExtension(Path).ToLowerInvariant() switch
            {
                ".png" or ".jpg" or ".jpeg" or ".gif" or ".bmp" or ".webp" or ".heic" => "",
                ".mp4" or ".mov" or ".mkv" or ".avi" or ".webm" => "",
                ".mp3" or ".flac" or ".wav" or ".m4a" or ".aac" or ".ogg" => "",
                ".zip" or ".rar" or ".7z" or ".tar" or ".gz" => "",
                ".pdf" or ".doc" or ".docx" or ".txt" or ".md" or ".ppt" or ".pptx" or ".xls" or ".xlsx" => "",
                ".exe" or ".msi" => "",
                _ => "",
            };
        }
    }

    public string Detail
    {
        get
        {
            try
            {
                if (IsFolder) return "文件夹";
                var info = new FileInfo(Path);
                return info.Exists ? FormatSize(info.Length) : "文件已不存在";
            }
            catch
            {
                return "";
            }
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
