using System.Diagnostics;
using System.IO;
using System.Text.Encodings.Web;
using System.Text.Json;

namespace Island.Core;

/// <summary>
/// 把数据以 JSON 保存在 %APPDATA%\Island 下。写入先写临时文件再替换，断电也不会写坏。
/// </summary>
public static class JsonStore
{
    private static readonly JsonSerializerOptions Options = new()
    {
        WriteIndented = true,
        Encoder = JavaScriptEncoder.UnsafeRelaxedJsonEscaping, // 中文原样保存，方便手动查看
    };

    /// <summary>数据目录。可以用环境变量 ISLAND_DATA_DIR 改到别处（冒烟测试用它隔离真实数据）。</summary>
    public static string DataDirectory { get; } =
        Environment.GetEnvironmentVariable("ISLAND_DATA_DIR") is { Length: > 0 } custom
            ? custom
            : Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData), "Island");

    public static string PathOf(string fileName) => Path.Combine(DataDirectory, fileName);

    public static T Load<T>(string fileName) where T : new()
    {
        try
        {
            string path = PathOf(fileName);
            if (!File.Exists(path)) return new T();
            return JsonSerializer.Deserialize<T>(File.ReadAllText(path), Options) ?? new T();
        }
        catch (Exception ex)
        {
            Debug.WriteLine($"[Store] 读取 {fileName} 失败：{ex.Message}");
            return new T();
        }
    }

    public static void Save<T>(string fileName, T value)
    {
        try
        {
            Directory.CreateDirectory(DataDirectory);
            string path = PathOf(fileName);
            string temp = path + ".tmp";
            File.WriteAllText(temp, JsonSerializer.Serialize(value, Options));
            File.Move(temp, path, overwrite: true);
        }
        catch (Exception ex)
        {
            Debug.WriteLine($"[Store] 保存 {fileName} 失败：{ex.Message}");
        }
    }
}
