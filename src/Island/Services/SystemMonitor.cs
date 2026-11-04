using System.Net.NetworkInformation;
using System.Runtime.InteropServices;

namespace Island.Services;

public readonly record struct SystemSample(
    double CpuPercent,
    double MemoryPercent,
    ulong MemoryUsedBytes,
    ulong MemoryTotalBytes,
    double DownloadBytesPerSecond,
    double UploadBytesPerSecond);

/// <summary>
/// CPU、内存、网速采样。调用方按固定间隔调用 <see cref="Sample"/>，第一次调用只建立基线。
/// 只用 Win32 和 .NET 自带的接口，不依赖性能计数器（那个初始化很慢）。
/// </summary>
public sealed class SystemMonitor
{
    private ulong _lastIdle, _lastKernel, _lastUser;
    private long _lastReceived, _lastSent;
    private DateTime _lastSampleAt;
    private bool _hasBaseline;

    public SystemSample Sample()
    {
        DateTime now = DateTime.UtcNow;
        double seconds = _hasBaseline ? Math.Max(0.001, (now - _lastSampleAt).TotalSeconds) : 1;

        double cpu = SampleCpu();
        var (usedMem, totalMem, memPercent) = SampleMemory();
        var (received, sent) = SampleNetwork();

        double down = 0, up = 0;
        if (_hasBaseline)
        {
            down = Math.Max(0, received - _lastReceived) / seconds;
            up = Math.Max(0, sent - _lastSent) / seconds;
        }

        _lastReceived = received;
        _lastSent = sent;
        _lastSampleAt = now;
        _hasBaseline = true;

        return new SystemSample(cpu, memPercent, usedMem, totalMem, down, up);
    }

    private double SampleCpu()
    {
        if (!GetSystemTimes(out var idleTime, out var kernelTime, out var userTime)) return 0;

        ulong idle = idleTime.ToUInt64(), kernel = kernelTime.ToUInt64(), user = userTime.ToUInt64();
        double percent = 0;
        if (_hasBaseline)
        {
            // 内核时间里包含了空闲时间
            ulong idleDelta = idle - _lastIdle;
            ulong totalDelta = (kernel - _lastKernel) + (user - _lastUser);
            if (totalDelta > 0) percent = Math.Clamp(100.0 * (totalDelta - idleDelta) / totalDelta, 0, 100);
        }

        _lastIdle = idle;
        _lastKernel = kernel;
        _lastUser = user;
        return percent;
    }

    private static (ulong Used, ulong Total, double Percent) SampleMemory()
    {
        var status = new MEMORYSTATUSEX { dwLength = (uint)Marshal.SizeOf<MEMORYSTATUSEX>() };
        if (!GlobalMemoryStatusEx(ref status) || status.ullTotalPhys == 0) return (0, 0, 0);
        ulong used = status.ullTotalPhys - status.ullAvailPhys;
        return (used, status.ullTotalPhys, 100.0 * used / status.ullTotalPhys);
    }

    private static (long Received, long Sent) SampleNetwork()
    {
        long received = 0, sent = 0;
        try
        {
            foreach (var nic in NetworkInterface.GetAllNetworkInterfaces())
            {
                if (nic.OperationalStatus != OperationalStatus.Up) continue;
                if (nic.NetworkInterfaceType is NetworkInterfaceType.Loopback or NetworkInterfaceType.Tunnel) continue;

                var stats = nic.GetIPStatistics();
                received += stats.BytesReceived;
                sent += stats.BytesSent;
            }
        }
        catch
        {
            // 网卡在枚举过程中被禁用等情况，忽略这一次
        }
        return (received, sent);
    }

    public static string FormatSpeed(double bytesPerSecond) => bytesPerSecond switch
    {
        < 1024 => $"{bytesPerSecond:0} B/s",
        < 1024 * 1024 => $"{bytesPerSecond / 1024:0.0} KB/s",
        _ => $"{bytesPerSecond / 1024 / 1024:0.0} MB/s",
    };

    public static string FormatBytes(ulong bytes) => $"{bytes / 1024.0 / 1024 / 1024:0.0} GB";

    // ───────────────────────────── Win32 ─────────────────────────────

    [StructLayout(LayoutKind.Sequential)]
    private struct FILETIME
    {
        public uint dwLowDateTime;
        public uint dwHighDateTime;

        public readonly ulong ToUInt64() => ((ulong)dwHighDateTime << 32) | dwLowDateTime;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct MEMORYSTATUSEX
    {
        public uint dwLength;
        public uint dwMemoryLoad;
        public ulong ullTotalPhys;
        public ulong ullAvailPhys;
        public ulong ullTotalPageFile;
        public ulong ullAvailPageFile;
        public ulong ullTotalVirtual;
        public ulong ullAvailVirtual;
        public ulong ullAvailExtendedVirtual;
    }

    [DllImport("kernel32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool GetSystemTimes(out FILETIME idleTime, out FILETIME kernelTime, out FILETIME userTime);

    [DllImport("kernel32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool GlobalMemoryStatusEx(ref MEMORYSTATUSEX buffer);
}
