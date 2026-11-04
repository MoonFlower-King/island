using Island.Core;

namespace Island.Tests;

public class TranslationTests
{
    [Theory]
    [InlineData("", "https://api.openai.com/v1/chat/completions")]
    [InlineData("https://api.deepseek.com", "https://api.deepseek.com/chat/completions")]
    [InlineData("https://api.deepseek.com/", "https://api.deepseek.com/chat/completions")]
    [InlineData("  https://api.moonshot.cn/v1  ", "https://api.moonshot.cn/v1/chat/completions")]
    [InlineData("http://localhost:11434/v1/chat/completions", "http://localhost:11434/v1/chat/completions")]
    public void BuildsOpenAIUrl(string baseUrl, string expected) =>
        Assert.Equal(expected, ApiEndpoint.OpenAIChatUrl(baseUrl));

    [Theory]
    [InlineData("", "https://api.anthropic.com")]
    [InlineData("https://api.anthropic.com/", "https://api.anthropic.com")]
    [InlineData("https://proxy.example.com/v1", "https://proxy.example.com")]
    [InlineData("https://proxy.example.com/v1/messages", "https://proxy.example.com")]
    [InlineData("https://api.deepseek.com/anthropic", "https://api.deepseek.com/anthropic")]
    public void BuildsAnthropicSdkBase(string baseUrl, string expected) =>
        Assert.Equal(expected, ApiEndpoint.AnthropicSdkBase(baseUrl));

    [Fact]
    public void DescribesFinalRequestUrl()
    {
        Assert.Equal("https://api.anthropic.com/v1/messages", ApiEndpoint.Describe(ApiFormat.Anthropic, ""));
        Assert.Equal("https://api.openai.com/v1/chat/completions", ApiEndpoint.Describe(ApiFormat.OpenAI, null));
    }

    [Theory]
    [InlineData("", true)]
    [InlineData("https://api.deepseek.com", true)]
    [InlineData("http://127.0.0.1:1234/v1", true)]
    [InlineData("api.deepseek.com", false)]
    [InlineData("ftp://example.com", false)]
    [InlineData("not a url", false)]
    public void ValidatesUrl(string url, bool valid) => Assert.Equal(valid, ApiEndpoint.IsValid(url));

    [Theory]
    [InlineData("http://localhost:11434/v1", true)]
    [InlineData("http://127.0.0.1:1234", true)]
    [InlineData("https://api.openai.com/v1", false)]
    [InlineData("", false)]
    public void DetectsLocalServices(string url, bool loopback) => Assert.Equal(loopback, ApiEndpoint.IsLoopback(url));

    [Fact]
    public void OnlyOfficialAnthropicGetsNewParameters()
    {
        Assert.True(ApiEndpoint.IsOfficialAnthropic(""));
        Assert.True(ApiEndpoint.IsOfficialAnthropic("https://api.anthropic.com/v1"));
        Assert.False(ApiEndpoint.IsOfficialAnthropic("https://api.deepseek.com/anthropic"));
    }

    [Fact]
    public void PresetsAreWellFormed()
    {
        Assert.Equal(ProviderPresets.All.Count, ProviderPresets.All.Select(p => p.Id).Distinct().Count());
        foreach (ProviderPreset preset in ProviderPresets.All.Where(p => p.Id != ProviderPresets.CustomId))
        {
            Assert.True(ApiEndpoint.IsValid(preset.BaseUrl) && preset.BaseUrl.Length > 0, preset.Id);
            Assert.False(string.IsNullOrWhiteSpace(preset.Model), preset.Id);
        }
        Assert.False(ProviderPresets.Find("ollama").NeedsKey);
        Assert.True(ProviderPresets.Find("deepseek").NeedsKey);
        Assert.Same(ProviderPresets.Anthropic, ProviderPresets.Find("no-such-provider"));
    }

    [Fact]
    public void ParsesOpenAIStream()
    {
        Assert.True(OpenAIStream.TryParseLine("data: {\"choices\":[{\"delta\":{\"content\":\"你好\"}}]}", out var text, out var error));
        Assert.Equal("你好", text);
        Assert.Null(error);

        // 角色、空内容、思考内容、注释行都不产生译文
        Assert.True(OpenAIStream.TryParseLine("data: {\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}", out text, out _));
        Assert.Null(text);
        Assert.True(OpenAIStream.TryParseLine("data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"嗯\",\"content\":null}}]}", out text, out _));
        Assert.Null(text);
        Assert.True(OpenAIStream.TryParseLine(": keep-alive", out text, out _));
        Assert.Null(text);
        Assert.True(OpenAIStream.TryParseLine("", out text, out _));
        Assert.True(OpenAIStream.TryParseLine("data: {\"choices\":[]}", out text, out _));
        Assert.Null(text);

        Assert.True(OpenAIStream.TryParseLine("data: {\"error\":{\"message\":\"quota exceeded\"}}", out _, out error));
        Assert.Equal("quota exceeded", error);

        Assert.False(OpenAIStream.TryParseLine("data: [DONE]", out _, out _));
    }

    [Fact]
    public void ReadsErrorBodies()
    {
        Assert.Equal("Invalid API key", OpenAIStream.ErrorFromBody("{\"error\":{\"message\":\"Invalid API key\",\"type\":\"auth\"}}"));
        Assert.Equal("bad", OpenAIStream.ErrorFromBody("{\"error\":\"bad\"}"));
        Assert.Equal("oops", OpenAIStream.ErrorFromBody("{\"message\":\"oops\"}"));
        Assert.Null(OpenAIStream.ErrorFromBody("<html>502</html>"));
    }

    [Fact]
    public void ProfilesAreKeptPerProviderAndKeysAreEncrypted()
    {
        var settings = new AppSettings();
        TranslateProfile deepseek = settings.ProfileFor("deepseek");
        Assert.Equal(ApiFormat.OpenAI, deepseek.Format);
        Assert.Equal("deepseek-chat", deepseek.Model);

        settings.SetApiKey("deepseek", "sk-deepseek-123456");
        Assert.Equal("sk-deepseek-123456", settings.GetApiKey("deepseek"));
        Assert.DoesNotContain("123456", deepseek.ProtectedKey!);
        Assert.False(settings.HasStoredApiKey("kimi"));

        // 未知 Id 按自定义处理
        Assert.Equal("", settings.ProfileFor("my-proxy").BaseUrl);
    }

    [Fact]
    public void MigratesLegacyClaudeSettings()
    {
        var old = new AppSettings();
        old.SetApiKey("anthropic", "sk-ant-legacy-key");
        string legacyKey = old.ProfileFor("anthropic").ProtectedKey!;

        var settings = new AppSettings { TranslateModel = "claude-sonnet-5-5", ProtectedApiKey = legacyKey };
        Assert.True(settings.MigrateLegacy());
        Assert.Equal("claude-sonnet-5-5", settings.ProfileFor("anthropic").Model);
        Assert.Equal("sk-ant-legacy-key", settings.GetApiKey("anthropic"));
        Assert.Null(settings.TranslateModel);
        Assert.Null(settings.ProtectedApiKey);
        Assert.False(settings.MigrateLegacy());
    }
}
