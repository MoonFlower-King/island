using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;
using System.Windows.Threading;
using Island.Services;

namespace Island.Views.Panels;

public partial class TranslatePanel : UserControl
{
    private TranslationService? _service;
    private CancellationTokenSource? _running;

    public TranslatePanel() => InitializeComponent();

    public void Attach(TranslationService service)
    {
        _service = service;
        ShowKeySetup(!service.HasApiKey);
    }

    /// <summary>切到翻译标签时调用：把焦点放到输入框，方便直接打字。</summary>
    public void FocusInput() =>
        Dispatcher.BeginInvoke(DispatcherPriority.Input, new Action(() =>
        {
            if (KeySetup.Visibility == Visibility.Visible) KeyInput.Focus();
            else Input.Focus();
        }));

    private void ShowKeySetup(bool show, string? error = null)
    {
        KeySetup.Visibility = show ? Visibility.Visible : Visibility.Collapsed;
        Main.Visibility = show ? Visibility.Collapsed : Visibility.Visible;
        KeyError.Text = error ?? "";
        KeyError.Visibility = string.IsNullOrEmpty(error) ? Visibility.Collapsed : Visibility.Visible;
    }

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
        Status.Text = "翻译中…";
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
            if (ex.InvalidKey) ShowKeySetup(true, ex.Message);
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

    private void OnKeyInputKeyDown(object sender, KeyEventArgs e)
    {
        if (e.Key == Key.Enter)
        {
            e.Handled = true;
            SaveKey();
        }
    }

    private void OnSaveKey(object sender, RoutedEventArgs e) => SaveKey();

    private void SaveKey()
    {
        string key = KeyInput.Text.Trim();
        if (key.Length < 10)
        {
            ShowKeySetup(true, "看起来不是有效的 API Key");
            return;
        }

        _service?.SetApiKey(key);
        KeyInput.Clear();
        ShowKeySetup(false);
        Status.Text = "API Key 已保存";
        Input.Focus();
    }
}
