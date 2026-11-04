using System.Runtime.InteropServices;
using System.Windows.Threading;
using Island.Core;
using Island.Interop;

namespace Island.Services;

/// <summary>音量快照：0~1 的音量和是否静音。</summary>
public readonly record struct VolumeLevel(double Level, bool Muted);

/// <summary>
/// 系统主音量：监听默认输出设备的音量/静音变化，也可以设置音量。
/// 默认设备切换（比如插上耳机）时自动改为监听新设备。
/// Core Audio 的回调发生在后台线程，这里统一转回界面线程再触发 <see cref="Changed"/>。
/// </summary>
public sealed class VolumeService : IDisposable
{
    private static readonly Guid AudioEndpointVolumeId = typeof(IAudioEndpointVolume).GUID;
    private const int ClsctxAll = 0x17;

    // 自己设置音量时带上这个上下文，回调里就能认出是自己改的（目前仍然显示提示，留作扩展）
    private static readonly Guid OwnContext = new("9B2F6C7E-4C52-4F4E-8E6A-2A0E5D9F3B11");

    private readonly Dispatcher _dispatcher;
    private readonly VolumeCallback _volumeCallback;
    private readonly DeviceCallback _deviceCallback;
    private IMMDeviceEnumerator? _enumerator;
    private IAudioEndpointVolume? _endpoint;
    private bool _disposed;

    public VolumeService(Dispatcher dispatcher)
    {
        _dispatcher = dispatcher;
        _volumeCallback = new VolumeCallback(this);
        _deviceCallback = new DeviceCallback(this);
    }

    /// <summary>是否找到了可用的输出设备。</summary>
    public bool IsAvailable => _endpoint is not null;

    /// <summary>音量或静音状态变化（界面线程）。</summary>
    public event Action<VolumeLevel>? Changed;

    public void Start()
    {
        try
        {
            _enumerator = (IMMDeviceEnumerator)new MMDeviceEnumeratorComObject();
            _enumerator.RegisterEndpointNotificationCallback(_deviceCallback);
            AttachDefaultDevice();
        }
        catch (Exception ex)
        {
            Log.Error("初始化音量监听失败", ex);
        }
    }

    public VolumeLevel? Current
    {
        get
        {
            if (_endpoint is null) return null;
            try
            {
                _endpoint.GetMasterVolumeLevelScalar(out float level);
                _endpoint.GetMute(out bool muted);
                return new VolumeLevel(level, muted);
            }
            catch
            {
                return null;
            }
        }
    }

    /// <summary>设置主音量（0~1）。调大音量时顺便取消静音。</summary>
    public void SetLevel(double level)
    {
        if (_endpoint is null) return;
        try
        {
            Guid context = OwnContext;
            _endpoint.SetMasterVolumeLevelScalar((float)Math.Clamp(level, 0, 1), ref context);
            if (level > 0)
            {
                _endpoint.GetMute(out bool muted);
                if (muted) _endpoint.SetMute(false, ref context);
            }
        }
        catch (Exception ex)
        {
            Log.Error("设置音量失败", ex);
        }
    }

    private const double VolumeStep = 0.02;
    private const int WheelNotch = 120;
    private int _wheelRemainder;

    /// <summary>
    /// 按滚轮调整音量：每一格 2%。精确触控板一次只报很小的滚动量，这里累积到满一格才调，
    /// 否则轻轻一划音量就冲到底了。
    /// </summary>
    public void Nudge(int wheelDelta)
    {
        if (Current is not { } current) return;

        _wheelRemainder += wheelDelta;
        int notches = _wheelRemainder / WheelNotch;
        if (notches == 0) return;
        _wheelRemainder -= notches * WheelNotch;

        SetLevel(StepVolume(current.Level, notches));
    }

    /// <summary>
    /// 先把当前音量按滚动方向对齐到 2% 的整数倍，再走 <paramref name="notches"/> 格。
    /// 例如 37% 向上一格是 38%、向下一格是 36%，之后都是整齐的偶数。
    /// </summary>
    public static double StepVolume(double level, int notches)
    {
        if (notches == 0) return level;
        double units = level / VolumeStep;
        double aligned = notches > 0 ? Math.Floor(units + 1e-6) : Math.Ceiling(units - 1e-6);
        return Math.Clamp((aligned + notches) * VolumeStep, 0, 1);
    }

    private void AttachDefaultDevice()
    {
        DetachEndpoint();
        if (_enumerator is null || _disposed) return;

        // 没有输出设备时返回 E_NOTFOUND（0x80070490），属于正常情况
        if (_enumerator.GetDefaultAudioEndpoint(EDataFlow.Render, ERole.Multimedia, out IMMDevice? device) != 0 ||
            device is null)
        {
            Log.Info("没有找到默认音频输出设备");
            return;
        }

        try
        {
            Guid iid = AudioEndpointVolumeId;
            if (device.Activate(ref iid, ClsctxAll, IntPtr.Zero, out object? instance) != 0 ||
                instance is not IAudioEndpointVolume endpoint)
                return;

            endpoint.RegisterControlChangeNotify(_volumeCallback);
            _endpoint = endpoint;
        }
        finally
        {
            Marshal.ReleaseComObject(device);
        }
    }

    private void DetachEndpoint()
    {
        if (_endpoint is null) return;
        try
        {
            _endpoint.UnregisterControlChangeNotify(_volumeCallback);
            Marshal.ReleaseComObject(_endpoint);
        }
        catch
        {
            // 设备已经被拔掉了，忽略
        }
        _endpoint = null;
    }

    internal void OnVolumeNotify(VolumeLevel level) =>
        _dispatcher.BeginInvoke(new Action(() =>
        {
            if (!_disposed) Changed?.Invoke(level);
        }));

    internal void OnDefaultDeviceChanged() =>
        _dispatcher.BeginInvoke(new Action(() =>
        {
            if (_disposed) return;
            AttachDefaultDevice();
            if (Current is { } level) Changed?.Invoke(level);
        }));

    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;
        DetachEndpoint();
        if (_enumerator is not null)
        {
            try
            {
                _enumerator.UnregisterEndpointNotificationCallback(_deviceCallback);
                Marshal.ReleaseComObject(_enumerator);
            }
            catch
            {
                // 退出时忽略
            }
            _enumerator = null;
        }
    }
}

// ───────────────────────────── COM 回调 ─────────────────────────────
// Core Audio 通过这两个对象回调我们（在后台线程上），它们只负责转交给 VolumeService。

[ComVisible(true)]
internal sealed class VolumeCallback(VolumeService owner) : IAudioEndpointVolumeCallback
{
    public int OnNotify(IntPtr notificationData)
    {
        if (notificationData == IntPtr.Zero) return 0;
        var data = Marshal.PtrToStructure<AudioVolumeNotificationData>(notificationData);
        owner.OnVolumeNotify(new VolumeLevel(data.MasterVolume, data.Muted));
        return 0;
    }
}

[ComVisible(true)]
internal sealed class DeviceCallback(VolumeService owner) : IMMNotificationClient
{
    public int OnDeviceStateChanged(string deviceId, int newState) => 0;
    public int OnDeviceAdded(string deviceId) => 0;
    public int OnDeviceRemoved(string deviceId) => 0;
    public int OnPropertyValueChanged(string deviceId, PropertyKey key) => 0;

    public int OnDefaultDeviceChanged(EDataFlow flow, ERole role, string? defaultDeviceId)
    {
        if (flow == EDataFlow.Render && role == ERole.Multimedia) owner.OnDefaultDeviceChanged();
        return 0;
    }
}
