using System.Diagnostics;
using System.IO;

namespace Island.Core;

/// <summary>
/// 极简日志：写到 %APPDATA%\Island\island.log，超过 1 MB 自动轮换。
/// 出问题时用户可以把这个文件发过来。
/// </summary>
public static class Log
{
    private const long MaxBytes = 1024 * 1024;
    private static readonly object Gate = new();

    public static string FilePath => JsonStore.PathOf("island.log");

    public static void Info(string message) => Write("INFO", message);

    public static void Error(string message, Exception? ex = null) =>
        Write("ERROR", ex is null ? message : $"{message}{Environment.NewLine}{ex}");

    private static void Write(string level, string message)
    {
        string line = $"{DateTime.Now:yyyy-MM-dd HH:mm:ss.fff} [{level}] {message}{Environment.NewLine}";
        Debug.Write(line);

        try
        {
            lock (Gate)
            {
                Directory.CreateDirectory(JsonStore.DataDirectory);
                var info = new FileInfo(FilePath);
                if (info.Exists && info.Length > MaxBytes)
                    File.Move(FilePath, FilePath + ".old", overwrite: true);
                File.AppendAllText(FilePath, line);
            }
        }
        catch
        {
            // 日志写不进去也不能影响程序本身
        }
    }
}
