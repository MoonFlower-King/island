using System.Text.Json;
using System.Text.Json.Serialization;

namespace Island.Core;

/// <summary>翻译接口的请求格式。</summary>
[JsonConverter(typeof(JsonStringEnumConverter))]
public enum ApiFormat
{
    /// <summary>Anthropic Messages API（POST /v1/messages）。</summary>
    Anthropic,

    /// <summary>OpenAI Chat Completions（POST /chat/completions），大多数国内服务商和本地模型都兼容。</summary>
    OpenAI,
}

/// <summary>
/// 一个常用的翻译服务：选中后自动填好格式、请求地址和推荐模型，用户只需要填 Key。
/// <see cref="Id"/> 用来在 settings.json 里区分每个服务各自的配置。
/// </summary>
public sealed record ProviderPreset(
    string Id,
    string Name,
    ApiFormat Format,
    string BaseUrl,
    string Model,
    string KeyHint,
    string KeyPage = "")
{
    /// <summary>本地模型（如 Ollama）不需要 Key。</summary>
    public bool NeedsKey => !ApiEndpoint.IsLoopback(BaseUrl);
}

public static class ProviderPresets
{
    public const string CustomId = "custom";

    /// <summary>官方 Claude，默认选项。</summary>
    public static ProviderPreset Anthropic { get; } = new(
        "anthropic", "Claude", ApiFormat.Anthropic, ApiEndpoint.AnthropicDefaultBaseUrl, "claude-opus-5-5",
        "sk-ant-...", "https://console.anthropic.com/");

    /// <summary>自定义：格式、地址、模型全部自己填，用于代理、中转或其他兼容服务。</summary>
    public static ProviderPreset Custom { get; } = new(
        CustomId, "自定义", ApiFormat.OpenAI, "", "", "API Key（本地服务可留空）");

    public static IReadOnlyList<ProviderPreset> All { get; } =
    [
        Anthropic,
        new("openai", "OpenAI", ApiFormat.OpenAI, ApiEndpoint.OpenAIDefaultBaseUrl, "gpt-4o-mini",
            "sk-...", "https://platform.openai.com/api-keys"),
        new("deepseek", "DeepSeek", ApiFormat.OpenAI, "https://api.deepseek.com", "deepseek-chat",
            "sk-...", "https://platform.deepseek.com/api_keys"),
        new("qwen", "通义千问", ApiFormat.OpenAI, "https://dashscope.aliyuncs.com/compatible-mode/v1", "qwen-plus",
            "sk-...", "https://bailian.console.aliyun.com/"),
        new("kimi", "Kimi", ApiFormat.OpenAI, "https://api.moonshot.cn/v1", "moonshot-v1-8k",
            "sk-...", "https://platform.moonshot.cn/console/api-keys"),
        new("zhipu", "智谱", ApiFormat.OpenAI, "https://open.bigmodel.cn/api/paas/v4", "glm-4-flash",
            "API Key", "https://open.bigmodel.cn/usercenter/apikeys"),
        new("ollama", "Ollama 本地", ApiFormat.OpenAI, "http://localhost:11434/v1", "qwen2.5",
            "本地模型不需要 Key"),
        Custom,
    ];

    public static ProviderPreset Find(string? id) =>
        All.FirstOrDefault(p => string.Equals(p.Id, id, StringComparison.OrdinalIgnoreCase)) ?? Anthropic;
}

/// <summary>根据用户填的地址算出真正请求的 URL。用户可能填根地址、带 /v1 的地址，或者完整的接口地址。</summary>
public static class ApiEndpoint
{
    public const string AnthropicDefaultBaseUrl = "https://api.anthropic.com";
    public const string OpenAIDefaultBaseUrl = "https://api.openai.com/v1";

    public static string DefaultBaseUrl(ApiFormat format) =>
        format == ApiFormat.Anthropic ? AnthropicDefaultBaseUrl : OpenAIDefaultBaseUrl;

    /// <summary>留空时用格式的默认地址；去掉首尾空白和末尾的 /。</summary>
    public static string Clean(ApiFormat format, string? baseUrl)
    {
        string url = (baseUrl ?? "").Trim().TrimEnd('/');
        return url.Length == 0 ? DefaultBaseUrl(format) : url;
    }

    /// <summary>
    /// Anthropic SDK 需要的根地址（SDK 自己会加 /v1/messages）。
    /// 用户填了 …/v1 或完整的 …/v1/messages 也能用。
    /// </summary>
    public static string AnthropicSdkBase(string? baseUrl)
    {
        string url = Clean(ApiFormat.Anthropic, baseUrl);
        foreach (string suffix in new[] { "/v1/messages", "/messages", "/v1" })
        {
            if (url.EndsWith(suffix, StringComparison.OrdinalIgnoreCase))
                return url[..^suffix.Length];
        }
        return url;
    }

    /// <summary>OpenAI 格式的完整请求地址：填了完整地址就原样用，否则补上 /chat/completions。</summary>
    public static string OpenAIChatUrl(string? baseUrl)
    {
        string url = Clean(ApiFormat.OpenAI, baseUrl);
        return url.EndsWith("/chat/completions", StringComparison.OrdinalIgnoreCase) ? url : url + "/chat/completions";
    }

    /// <summary>界面上给用户看的“实际请求地址”。</summary>
    public static string Describe(ApiFormat format, string? baseUrl) =>
        format == ApiFormat.Anthropic ? AnthropicSdkBase(baseUrl) + "/v1/messages" : OpenAIChatUrl(baseUrl);

    /// <summary>地址是否像样：必须是 http(s) 的绝对地址。</summary>
    public static bool IsValid(string? baseUrl)
    {
        string url = (baseUrl ?? "").Trim();
        if (url.Length == 0) return true; // 留空 = 默认地址
        return Uri.TryCreate(url, UriKind.Absolute, out Uri? uri) &&
               (uri.Scheme == Uri.UriSchemeHttps || uri.Scheme == Uri.UriSchemeHttp) &&
               !string.IsNullOrEmpty(uri.Host);
    }

    /// <summary>本机地址（Ollama、LM Studio 等本地服务）。</summary>
    public static bool IsLoopback(string? baseUrl) =>
        Uri.TryCreate((baseUrl ?? "").Trim(), UriKind.Absolute, out Uri? uri) &&
        (uri.IsLoopback || uri.Host.Equals("localhost", StringComparison.OrdinalIgnoreCase));

    /// <summary>是不是 Anthropic 官方接口（官方接口才发 effort 之类的新参数，第三方兼容接口不一定认）。</summary>
    public static bool IsOfficialAnthropic(string? baseUrl) =>
        Uri.TryCreate(AnthropicSdkBase(baseUrl), UriKind.Absolute, out Uri? uri) &&
        uri.Host.Equals("api.anthropic.com", StringComparison.OrdinalIgnoreCase);
}

/// <summary>某一个翻译服务的配置。每个服务各存一份，来回切换时不用重新填。</summary>
public sealed class TranslateProfile
{
    public ApiFormat Format { get; set; } = ApiFormat.Anthropic;

    /// <summary>请求地址；留空表示格式的默认地址。</summary>
    public string BaseUrl { get; set; } = "";

    public string Model { get; set; } = "";

    /// <summary>用 Windows DPAPI 加密后的 Key。</summary>
    public string? ProtectedKey { get; set; }

    public static TranslateProfile FromPreset(ProviderPreset preset) => new()
    {
        Format = preset.Format,
        BaseUrl = preset.BaseUrl,
        Model = preset.Model,
    };
}

/// <summary>OpenAI 格式流式响应（SSE）的解析，独立出来方便测试。</summary>
public static class OpenAIStream
{
    /// <summary>
    /// 解析一行 SSE。返回 false 表示流结束（[DONE]）。<paramref name="text"/> 是这一行带来的译文片段，可能为空。
    /// </summary>
    public static bool TryParseLine(string line, out string? text, out string? error)
    {
        text = null;
        error = null;
        if (!line.StartsWith("data:", StringComparison.Ordinal)) return true; // 注释、event: 行、空行

        string data = line[5..].Trim();
        if (data == "[DONE]") return false;
        if (data.Length == 0) return true;

        try
        {
            using var doc = JsonDocument.Parse(data);
            var root = doc.RootElement;
            if (root.TryGetProperty("error", out var err))
            {
                error = ErrorMessage(err);
                return true;
            }
            if (root.TryGetProperty("choices", out var choices) && choices.ValueKind == JsonValueKind.Array &&
                choices.GetArrayLength() > 0 &&
                choices[0].TryGetProperty("delta", out var delta) &&
                delta.TryGetProperty("content", out var content) &&
                content.ValueKind == JsonValueKind.String)
            {
                text = content.GetString();
            }
        }
        catch (JsonException)
        {
            // 个别服务会夹带非 JSON 的心跳行，忽略
        }
        return true;
    }

    /// <summary>从错误响应体里取出可读的错误信息（OpenAI 和 Anthropic 都是 {"error": {"message": ...}}）。</summary>
    public static string? ErrorFromBody(string body)
    {
        try
        {
            using var doc = JsonDocument.Parse(body);
            if (doc.RootElement.TryGetProperty("error", out var err)) return ErrorMessage(err);
            if (doc.RootElement.TryGetProperty("message", out var msg) && msg.ValueKind == JsonValueKind.String)
                return msg.GetString();
        }
        catch (JsonException)
        {
        }
        return null;
    }

    private static string? ErrorMessage(JsonElement err) => err.ValueKind switch
    {
        JsonValueKind.String => err.GetString(),
        JsonValueKind.Object when err.TryGetProperty("message", out var m) &&
                                                   m.ValueKind == JsonValueKind.String => m.GetString(),
        _ => err.ToString(),
    };
}
