using System.Threading;
using System.Windows;
using Island.Views;

namespace Island;

public partial class App : Application
{
    private const string MutexName = "Island.SingleInstance.{6C1D9A0E-4B9B-4C1E-9F3A-2E7C5B8D1A42}";
    private Mutex? _singleInstance;

    protected override void OnStartup(StartupEventArgs e)
    {
        // 单实例：已经有一个岛在运行就直接退出
        _singleInstance = new Mutex(initiallyOwned: true, MutexName, out bool createdNew);
        if (!createdNew)
        {
            Shutdown();
            return;
        }

        base.OnStartup(e);

        var island = new IslandWindow();
        island.Show();
    }

    protected override void OnExit(ExitEventArgs e)
    {
        _singleInstance?.Dispose();
        base.OnExit(e);
    }
}
