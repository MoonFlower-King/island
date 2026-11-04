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

    /// <summary>当前使用的翻译服务（<see cref="ProviderPresets"/> 里的 Id）。</summary>
    public string TranslateProvider { get; set; } = ProviderPresets.Anthropic.Id;

    /// <summary>每个翻译服务各自的格式、地址、模型和加密的 Key。</summary>
    public Dictionary<string, TranslateProfile> TranslateProfiles { get; set; } = new(StringComparer.OrdinalIgnoreCase);

    /// <summary>旧版本（1.2 及以前）的模型设置，读到后迁移进 <see cref="TranslateProfiles"/>。</summary>
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public string? TranslateModel { get; set; }

    /// <summary>旧版本的 Key，读到后迁移进 <see cref="TranslateProfiles"/>。</summary>
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public string? ProtectedApiKey { get; set; }

    /// <summary>外观主题。</summary>
    public ThemeSettings Theme { get; set; } = new();

    /// <summary>调节音量或亮度时在岛上显示提示。</summary>
    public bool ShowLevelHud { get; set; } = true;

    /// <summary>待机时缩成顶端的一条细线（否则显示带时间的小胶囊）。</summary>
    public bool IdleAsLine { get; set; } = true;

    /// <summary>播放音乐时也只显示细线（细线变成封面的主题色），不撑成音乐胶囊。</summary>
    public bool MediaAsLine { get; set; }

    /// <summary>岛贴在屏幕的哪条边上。</summary>
    [JsonConverter(typeof(JsonStringEnumConverter))]
    public DockEdge Dock { get; set; } = DockEdge.Top;

    /// <summary>剪贴板历史是否保存到磁盘（关闭后只保留在内存里，退出即清空）。</summary>
    public bool PersistClipboardHistory { get; set; } = true;

    /// <summary>某个服务的配置；还没配过就用预设的默认值。</summary>
    public TranslateProfile ProfileFor(string providerId)
    {
        if (TranslateProfiles.TryGetValue(providerId, out var profile)) return profile;
        ProviderPreset preset = ProviderPresets.Find(providerId);
        profile = TranslateProfile.FromPreset(
            preset.Id.Equals(providerId, StringComparison.OrdinalIgnoreCase) ? preset : ProviderPresets.Custom);
        TranslateProfiles[providerId] = profile;
        return profile;
    }

    /// <summary>
    /// 某个服务的 Key：先用填写保存的；没填的话，官方 Claude 接口读环境变量 ANTHROPIC_API_KEY，
    /// 官方 OpenAI 接口读 OPENAI_API_KEY。
    /// </summary>
    public string? GetApiKey(string providerId)
    {
        TranslateProfile profile = ProfileFor(providerId);
        string? stored = Unprotect(profile.ProtectedKey);
        if (!string.IsNullOrWhiteSpace(stored)) return stored;

        string? env = profile.Format switch
        {
            ApiFormat.Anthropic when ApiEndpoint.IsOfficialAnthropic(profile.BaseUrl) => "ANTHROPIC_API_KEY",
            ApiFormat.OpenAI when ApiEndpoint.Clean(ApiFormat.OpenAI, profile.BaseUrl)
                .Equals(ApiEndpoint.OpenAIDefaultBaseUrl, StringComparison.OrdinalIgnoreCase) => "OPENAI_API_KEY",
            _ => null,
        };
        return env is null ? null : Environment.GetEnvironmentVariable(env) is { Length: > 0 } value ? value : null;
    }

    public bool HasStoredApiKey(string providerId) => !string.IsNullOrEmpty(ProfileFor(providerId).ProtectedKey);

    public void SetApiKey(string providerId, string? key) => ProfileFor(providerId).ProtectedKey = Protect(key);

    /// <summary>把旧版本的单一 Claude 配置搬进新的按服务保存的配置里。</summary>
    internal bool MigrateLegacy()
    {
        if (TranslateModel is null && ProtectedApiKey is null) return false;

        TranslateProfile claude = ProfileFor(ProviderPresets.Anthropic.Id);
        if (!string.IsNullOrWhiteSpace(TranslateModel)) claude.Model = TranslateModel;
        if (!string.IsNullOrEmpty(ProtectedApiKey) && string.IsNullOrEmpty(claude.ProtectedKey))
            claude.ProtectedKey = ProtectedApiKey;

        TranslateModel = null;
        ProtectedApiKey = null;
        return true;
    }

    private static string? Protect(string? value) =>
        string.IsNullOrWhiteSpace(value)
            ? null
            : Convert.ToBase64String(ProtectedData.Protect(
                Encoding.UTF8.GetBytes(value.Trim()), null, DataProtectionScope.CurrentUser));

    private static string? Unprotect(string? value)
    {
        if (string.IsNullOrEmpty(value)) return null;
        try
        {
            byte[] data = ProtectedData.Unprotect(Convert.FromBase64String(value), null, DataProtectionScope.CurrentUser);
            return Encoding.UTF8.GetString(data);
        }
        catch
        {
            return null;
        }
    }

    public static AppSettings Load()
    {
        var settings = JsonStore.Load<AppSettings>(FileName);
        // 字典从 JSON 读出来时不带忽略大小写的比较器，重新包一层
        settings.TranslateProfiles = new(settings.TranslateProfiles ?? new(), StringComparer.OrdinalIgnoreCase);
        if (settings.MigrateLegacy()) settings.Save();
        return settings;
    }

    public void Save() => JsonStore.Save(FileName, this);
}
