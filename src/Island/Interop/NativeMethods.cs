using System.Runtime.InteropServices;

namespace Island.Interop;

/// <summary>
/// 岛窗口需要的少量 Win32 调用。
/// </summary>
internal static class NativeMethods
{
    public const int GWL_EXSTYLE = -20;

    /// <summary>不出现在任务栏和 Alt+Tab 中。</summary>
    public const long WS_EX_TOOLWINDOW = 0x00000080L;

    /// <summary>点击时不抢占焦点，避免打断用户正在输入的窗口。</summary>
    public const long WS_EX_NOACTIVATE = 0x08000000L;

    /// <summary>鼠标完全穿透（暂未使用，留给“全屏时让开”之类的功能）。</summary>
    public const long WS_EX_TRANSPARENT = 0x00000020L;

    public const long WS_EX_APPWINDOW = 0x00040000L;

    public const int WM_MOUSEACTIVATE = 0x0021;
    public const int MA_NOACTIVATE = 3;

    public const int WM_CLIPBOARDUPDATE = 0x031D;

    public static readonly IntPtr HWND_TOPMOST = new(-1);

    public const uint SWP_NOSIZE = 0x0001;
    public const uint SWP_NOMOVE = 0x0002;
    public const uint SWP_NOACTIVATE = 0x0010;
    public const uint SWP_FRAMECHANGED = 0x0020;

    [DllImport("user32.dll", EntryPoint = "GetWindowLongPtrW", SetLastError = true)]
    private static extern IntPtr GetWindowLongPtr64(IntPtr hWnd, int nIndex);

    [DllImport("user32.dll", EntryPoint = "SetWindowLongPtrW", SetLastError = true)]
    private static extern IntPtr SetWindowLongPtr64(IntPtr hWnd, int nIndex, IntPtr dwNewLong);

    [DllImport("user32.dll", EntryPoint = "GetWindowLongW", SetLastError = true)]
    private static extern int GetWindowLong32(IntPtr hWnd, int nIndex);

    [DllImport("user32.dll", EntryPoint = "SetWindowLongW", SetLastError = true)]
    private static extern int SetWindowLong32(IntPtr hWnd, int nIndex, int dwNewLong);

    [DllImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool SetWindowPos(
        IntPtr hWnd, IntPtr hWndInsertAfter, int x, int y, int cx, int cy, uint uFlags);

    [DllImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool AddClipboardFormatListener(IntPtr hwnd);

    [DllImport("user32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool RemoveClipboardFormatListener(IntPtr hwnd);

    [StructLayout(LayoutKind.Sequential)]
    public struct POINT
    {
        public int X;
        public int Y;
    }

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool GetCursorPos(out POINT point);

    [DllImport("user32.dll")]
    public static extern IntPtr GetForegroundWindow();

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool SetForegroundWindow(IntPtr hWnd);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool IsWindow(IntPtr hWnd);

    /// <summary>SHQueryUserNotificationState 的返回值中表示“用户正在全屏做事”的几种。</summary>
    private const int QUNS_BUSY = 2;                    // 有全屏程序（视频、F11 浏览器等）
    private const int QUNS_RUNNING_D3D_FULL_SCREEN = 3; // 全屏游戏
    private const int QUNS_PRESENTATION_MODE = 4;       // 演示模式（PPT 放映等）

    [DllImport("shell32.dll")]
    private static extern int SHQueryUserNotificationState(out int state);

    /// <summary>当前是否有全屏程序在前台。这时岛应该让开。</summary>
    public static bool IsFullscreenAppRunning()
    {
        try
        {
            if (SHQueryUserNotificationState(out int state) != 0) return false;
            return state is QUNS_BUSY or QUNS_RUNNING_D3D_FULL_SCREEN or QUNS_PRESENTATION_MODE;
        }
        catch
        {
            return false;
        }
    }

    public static long GetWindowLong(IntPtr hWnd, int nIndex) =>
        IntPtr.Size == 8
            ? GetWindowLongPtr64(hWnd, nIndex).ToInt64()
            : GetWindowLong32(hWnd, nIndex);

    public static void SetWindowLong(IntPtr hWnd, int nIndex, long value)
    {
        if (IntPtr.Size == 8)
            SetWindowLongPtr64(hWnd, nIndex, new IntPtr(value));
        else
            SetWindowLong32(hWnd, nIndex, unchecked((int)value));
    }

    /// <summary>
    /// 把窗口设置成“浮层”：工具窗口、不激活、始终置顶。
    /// </summary>
    public static void MakeOverlay(IntPtr hwnd)
    {
        long ex = GetWindowLong(hwnd, GWL_EXSTYLE);
        ex |= WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
        ex &= ~WS_EX_APPWINDOW;
        SetWindowLong(hwnd, GWL_EXSTYLE, ex);

        SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED);
    }

    /// <summary>
    /// 重新声明置顶。有些全屏程序或系统弹窗会把我们压下去，定期调用可以抢回来。
    /// </summary>
    public static void BringToTopmost(IntPtr hwnd) =>
        SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
}
