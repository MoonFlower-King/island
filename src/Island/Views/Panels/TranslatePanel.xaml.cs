using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;
using System.Windows.Threading;
using Island.Core;
using Island.Services;

namespace Island.Views.Panels;

public partial class TranslatePanel : UserControl
{
    private const string DefaultStatus = "中文 ⇄ English，其他语言 → 中文";

    private TranslationService? _service;
    private AppSettings? _settings;
    private CancellationTokenSource? _running;
    private readonly Dictionary<string, RadioButton> _chips = new(StringComparer.OrdinalIgnoreCase);
    private string _editingProvider = ProviderPresets.Anthropic.Id;
    private bool _loadingFields;

    public TranslatePanel()
    {
        InitializeComponent();
        foreach (ProviderPreset preset in ProviderPresets.All)
        {
            var chip = new RadioButton
            {
                Content = preset.Name,
                Tag = preset.Id,
                Style = (Style)Resources["ProviderChip"],
            };
            chip.Checked += OnProviderChecked;
            _chips[preset.Id] = chip;
            ProviderChips.Children.Add(chip);
        }
    }

    public void Attach(TranslationService service, AppSettings settings)
    {
        _service = service;
        _settings = settings;
        ShowSettings(!service.IsConfigured);
        ShowIdleStatus();
    }

    /// <summary>设置页是否打开。</summary>
    public bool IsShowingSettings => Settings.Visibility == Visibility.Visible;

    /// <summary>切到翻译标签时调用：把焦点放到输入框，方便直接打字。</summary>
    public void FocusInput() =>
        Dispatcher.BeginInvoke(DispatcherPriority.Input, new Action(() =>
        {
            if (!IsShowingSettings) Input.Focus();
            else if (KeyInput.IsEnabled && string.IsNullOrEmpty(KeyInput.Text)) KeyInput.Focus();
            else ModelInput.Focus();
        }));

    // ───────────────────────────── 翻译 ─────────────────────────────

    private void ShowIdleStatus() =>
        Status.Text = _service is null ? DefaultStatus : $"{_service.Summary}　{DefaultStatus}";

    private void OnInputKeyDown(object sender, KeyEventArgs e)
    {
        if (e.Key == Key.Enter && (Keyboard.Modifiers & ModifierKeys.Shift) == 0)
        {
            e.Handled = true;
            _ = TranslateAsync(Input.Text);
        }
    }

    private void OnTranslate(object sender, RoutedEventArgs e) => _ = TranslateAsync(Input.Text);

    private void OnPasteAndTranslate(object sender, RoutedEventArgs e)
    {
        string text;
        try
        {
            text = Clipboard.ContainsText() ? Clipboard.GetText() : "";
        }
        catch
        {
            text = "";
        }

        if (string.IsNullOrWhiteSpace(text))
        {
            Status.Text = "剪贴板里没有文字";
            return;
        }
        Input.Text = text;
        _ = TranslateAsync(text);
    }

    private void OnCopyResult(object sender, RoutedEventArgs e)
    {
        if (string.IsNullOrEmpty(Output.Text)) return;
        try
        {
            Clipboard.SetText(Output.Text);
            Status.Text = "译文已复制";
        }
        catch
        {
            Status.Text = "复制失败，请重试";
        }
    }

    private async Task TranslateAsync(string text)
    {
        if (_service is null || string.IsNullOrWhiteSpace(text)) return;

        // 再次点击时取消上一次还没结束的翻译
        _running?.Cancel();
        var cts = new CancellationTokenSource();
        _running = cts;

        Output.Clear();
        Status.Text = $"{_service.Provider.Name} 翻译中…";
        TranslateButton.IsEnabled = false;

        try
        {
            await _service.TranslateAsync(text.Trim(), chunk =>
            {
                if (cts.IsCancellationRequested) return;
                Output.AppendText(chunk);
                Output.ScrollToEnd();
            }, cts.Token);

            if (!cts.IsCancellationRequested) Status.Text = "完成";
        }
        catch (OperationCanceledException)
        {
            // 被新的翻译取代，什么都不用做
        }
        catch (TranslationException ex)
        {
            if (ex.NeedsSetup) ShowSettings(true, ex.Message);
            else Status.Text = ex.Message;
        }
        catch (Exception ex)
        {
            Status.Text = $"翻译失败：{ex.Message}";
        }
        finally
        {
            if (ReferenceEquals(_running, cts))
            {
                _running = null;
                TranslateButton.IsEnabled = true;
            }
            cts.Dispose();
        }
    }

    // ───────────────────────────── 设置 ─────────────────────────────

    private void OnOpenSettings(object sender, RoutedEventArgs e)
    {
        ShowSettings(true);
        FocusInput();
    }

    private void OnCloseSettings(object sender, RoutedEventArgs e) => ShowSettings(false);

    /// <summary>打开或关闭设置页。打开时载入当前服务的配置。</summary>
    public void ShowSettings(bool show, string? error = null)
    {
        Settings.Visibility = show ? Visibility.Visible : Visibility.Collapsed;
        Main.Visibility = show ? Visibility.Collapsed : Visibility.Visible;
        if (!show)
        {
            ShowIdleStatus();
            return;
        }

        // 还没配好时没有“返回”可言
        BackButton.Visibility = _service?.IsConfigured == true ? Visibility.Visible : Visibility.Collapsed;
        SelectProvider(_settings?.TranslateProvider ?? ProviderPresets.Anthropic.Id);
        if (error is not null) SetHint(error, isError: true);
    }

    /// <summary>选中一个服务（冒烟测试也用）。</summary>
    public void SelectProvider(string providerId)
    {
        ProviderPreset preset = ProviderPresets.Find(providerId);
        if (_chips.TryGetValue(preset.Id, out RadioButton? chip) && chip.IsChecked != true)
        {
            chip.IsChecked = true; // 触发 OnProviderChecked → LoadFields
            return;
        }
        LoadFields(preset.Id);
    }

    private void OnProviderChecked(object sender, RoutedEventArgs e)
    {
        if (sender is RadioButton { Tag: string id }) LoadFields(id);
    }

    /// <summary>把某个服务保存过的配置（没有就是预设值）填进输入框。</summary>
    private void LoadFields(string providerId)
    {
        if (_settings is null) return;
        _editingProvider = providerId;
        ProviderPreset preset = ProviderPresets.Find(providerId);
        TranslateProfile profile = _settings.ProfileFor(providerId);

        _loadingFields = true;
        (profile.Format == ApiFormat.Anthropic ? FormatAnthropic : FormatOpenAI).IsChecked = true;
        BaseUrlInput.Text = profile.BaseUrl;
        ModelInput.Text = profile.Model;
        KeyInput.Clear();
        _loadingFields = false;

        UpdateKeyPlaceholder(preset);
        UpdateEndpointHint();
    }

    private void UpdateKeyPlaceholder(ProviderPreset preset)
    {
        bool stored = _settings?.HasStoredApiKey(_editingProvider) == true;
        bool fromEnv = !stored && _settings?.GetApiKey(_editingProvider) is not null;
        KeyInput.Tag = stored ? "已保存，留空则不修改"
            : fromEnv ? "使用环境变量里的 Key"
            : ApiEndpoint.IsLoopback(BaseUrlInput.Text) ? "本地服务不需要 Key"
            : preset.KeyHint;
    }

    private ApiFormat SelectedFormat => FormatAnthropic.IsChecked == true ? ApiFormat.Anthropic : ApiFormat.OpenAI;

    private void OnFormatChecked(object sender, RoutedEventArgs e)
    {
        if (_loadingFields || _settings is null) return;
        // 自定义服务换了格式，默认地址也跟着换
        UpdateEndpointHint();
    }

    private void OnEndpointChanged(object sender, TextChangedEventArgs e)
    {
        if (_loadingFields) return;
        UpdateKeyPlaceholder(ProviderPresets.Find(_editingProvider));
        UpdateEndpointHint();
    }

    /// <summary>告诉用户最终会请求哪个地址，填错了一眼能看出来。</summary>
    private void UpdateEndpointHint()
    {
        if (!ApiEndpoint.IsValid(BaseUrlInput.Text))
        {
            SetHint("地址应该以 http:// 或 https:// 开头", isError: true);
            return;
        }
        SetHint("请求 " + ApiEndpoint.Describe(SelectedFormat, BaseUrlInput.Text), isError: false);
    }

    private void SetHint(string text, bool isError)
    {
        SettingsHint.Text = text;
        SettingsHint.ToolTip = text;
        if (isError) SettingsHint.Foreground = new System.Windows.Media.SolidColorBrush(
            System.Windows.Media.Color.FromRgb(0xFF, 0x69, 0x61));
        else SettingsHint.SetResourceReference(TextBlock.ForegroundProperty, "IslandTertiary");
    }

    private void OnSettingsKeyDown(object sender, KeyEventArgs e)
    {
        if (e.Key != Key.Enter) return;
        e.Handled = true;
        if (Save()) ShowSettings(false);
    }

    private void OnSave(object sender, RoutedEventArgs e)
    {
        if (Save())
        {
            ShowSettings(false);
            Status.Text = $"已切换到 {_service!.Summary}";
            Input.Focus();
        }
    }

    private async void OnTest(object sender, RoutedEventArgs e)
    {
        if (_service is null || !Save()) return;

        TestButton.IsEnabled = false;
        SetHint("正在测试…", isError: false);
        using var cts = new CancellationTokenSource(TimeSpan.FromSeconds(30));
        var reply = new System.Text.StringBuilder();
        try
        {
            await _service.TranslateAsync("Hello", chunk => reply.Append(chunk), cts.Token);
            string text = reply.ToString().Trim();
            SetHint(text.Length > 0 ? $"连接成功：Hello → {text}" : "连接成功，但没有返回内容", isError: text.Length == 0);
            BackButton.Visibility = Visibility.Visible;
        }
        catch (TranslationException ex)
        {
            SetHint(ex.Message, isError: true);
        }
        catch (OperationCanceledException)
        {
            SetHint("30 秒没有响应，请检查地址", isError: true);
        }
        catch (Exception ex)
        {
            SetHint($"测试失败：{ex.Message}", isError: true);
        }
        finally
        {
            TestButton.IsEnabled = true;
        }
    }

    /// <summary>校验并保存当前填写的配置。失败时在提示行说明原因。</summary>
    private bool Save()
    {
        if (_service is null) return false;

        string baseUrl = BaseUrlInput.Text.Trim();
        string model = ModelInput.Text.Trim();
        string key = KeyInput.Text.Trim();

        if (!ApiEndpoint.IsValid(baseUrl))
        {
            SetHint("地址应该以 http:// 或 https:// 开头", isError: true);
            return false;
        }
        if (model.Length == 0)
        {
            SetHint("请填写模型名称", isError: true);
            ModelInput.Focus();
            return false;
        }
        if (key.Length > 0 && key.Length < 8)
        {
            SetHint("看起来不是有效的 API Key", isError: true);
            return false;
        }

        _service.Configure(_editingProvider, SelectedFormat, baseUrl, model, key.Length > 0 ? key : null);
        KeyInput.Clear();
        UpdateKeyPlaceholder(ProviderPresets.Find(_editingProvider));

        if (!_service.IsConfigured)
        {
            SetHint($"请填写 {_service.Provider.Name} 的 API Key", isError: true);
            KeyInput.Focus();
            return false;
        }
        UpdateEndpointHint();
        return true;
    }
}
