using System.Security.Cryptography;
using System.Text;
using System.Text.Json.Serialization;

namespace Island.Core;

/// <summary>应用设置，保存在 %APPDATA%\Island\settings.json。</summary>
public sealed class AppSettings
{
    private const string FileName = "settings.json";

    /// <summary>上次打开的标签，下次展开时还停在这里。</summary>
    public string LastTab { get; set; } = nameof(IslandTab.Music);

    /// <summary>翻译使用的 Claude 模型。</summary>
    public string TranslateModel { get; set; } = "claude-opus-5-5";

    /// <summary>外观主题。</summary>
    public ThemeSettings Theme { get; set; } = new();

    /// <summary>待机时缩成顶端的一条细线（否则显示带时间的小胶囊）。</summary>
    public bool IdleAsLine { get; set; } = true;

    /// <summary>剪贴板历史是否保存到磁盘（关闭后只保留在内存里，退出即清空）。</summary>
    public bool PersistClipboardHistory { get; set; } = true;

    /// <summary>用 Windows DPAPI 加密后的 API Key，只有当前 Windows 用户能解密。</summary>
    public string? ProtectedApiKey { get; set; }

    [JsonIgnore]
    public string? ApiKey
    {
        get
        {
            string? env = Environment.GetEnvironmentVariable("ANTHROPIC_API_KEY");
            if (!string.IsNullOrWhiteSpace(env)) return env;
            if (string.IsNullOrEmpty(ProtectedApiKey)) return null;
            try
            {
                byte[] data = ProtectedData.Unprotect(
                    Convert.FromBase64String(ProtectedApiKey), null, DataProtectionScope.CurrentUser);
                return Encoding.UTF8.GetString(data);
            }
            catch
            {
                return null;
            }
        }
        set
        {
            ProtectedApiKey = string.IsNullOrWhiteSpace(value)
                ? null
                : Convert.ToBase64String(ProtectedData.Protect(
                    Encoding.UTF8.GetBytes(value.Trim()), null, DataProtectionScope.CurrentUser));
        }
    }

    public static AppSettings Load() => JsonStore.Load<AppSettings>(FileName);

    public void Save() => JsonStore.Save(FileName, this);
}
