using System.Threading;
using System.Windows;
using System.Windows.Threading;
using Island.Core;
using Island.Views;

namespace Island;

public partial class App : Application
{
    private const string MutexName = "Island.SingleInstance.{6C1D9A0E-4B9B-4C1E-9F3A-2E7C5B8D1A42}";
    private Mutex? _singleInstance;

    protected override void OnStartup(StartupEventArgs e)
    {
        // 冒烟测试：Island.exe --smoke-test <输出目录>，跑完所有界面后退出（CI 用）
        string? smokeOutput = null;
        int smokeIndex = Array.IndexOf(e.Args, "--smoke-test");
        if (smokeIndex >= 0)
            smokeOutput = smokeIndex + 1 < e.Args.Length ? e.Args[smokeIndex + 1] : "smoke-output";

        // 单实例：已经有一个岛在运行就直接退出（冒烟测试不受影响）
        if (smokeOutput is null)
        {
            _singleInstance = new Mutex(initiallyOwned: true, MutexName, out bool createdNew);
            if (!createdNew)
            {
                Shutdown();
                return;
            }
        }

        RegisterExceptionHandlers(exitOnError: smokeOutput is not null);
        base.OnStartup(e);

        Log.Info($"启动 Island {typeof(App).Assembly.GetName().Version}，系统 {Environment.OSVersion}");

        var island = new IslandWindow();
        island.Show();

        if (smokeOutput is not null) _ = island.RunSmokeTestAsync(smokeOutput);
    }

    protected override void OnExit(ExitEventArgs e)
    {
        Log.Info($"退出，代码 {e.ApplicationExitCode}");
        _singleInstance?.Dispose();
        base.OnExit(e);
    }

    /// <summary>
    /// 意外异常写进日志，尽量让岛继续运行，不要因为一个小功能出错整个消失。
    /// 冒烟测试时则直接以失败退出，让 CI 发现问题。
    /// </summary>
    private void RegisterExceptionHandlers(bool exitOnError)
    {
        DispatcherUnhandledException += (_, args) =>
        {
            Log.Error("界面线程未处理的异常", args.Exception);
            if (exitOnError)
            {
                Shutdown(1);
                return;
            }
            args.Handled = true;
        };

        AppDomain.CurrentDomain.UnhandledException += (_, args) =>
            Log.Error("未处理的异常", args.ExceptionObject as Exception);

        TaskScheduler.UnobservedTaskException += (_, args) =>
        {
            Log.Error("未观察到的任务异常", args.Exception);
            args.SetObserved();
        };
    }
}
