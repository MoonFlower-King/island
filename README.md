# Island

Windows 上的"灵动岛"：屏幕顶部正中的一个黑色胶囊，会根据正在发生的事情平滑地变形。

当前进度：胶囊窗口骨架 + 音乐播放控制（WPF + .NET 8）。

## 功能

- 透明、无边框、始终置顶的浮层窗口
  - 不出现在任务栏和 Alt+Tab 里（`WS_EX_TOOLWINDOW`）
  - 点击时不抢焦点（`WS_EX_NOACTIVATE` + `WM_MOUSEACTIVATE`）
  - 胶囊以外的透明区域鼠标直接穿透
- 四种状态，使用阻尼弹簧动画切换尺寸和圆角，内容交叉淡入淡出
  | 状态 | 尺寸 | 触发 |
  |---|---|---|
  | 待机 Idle | 140×34 | 默认，显示时间 |
  | 紧凑 Compact | 260×38 | 正在播放音乐，或其他常驻活动（演示：右键菜单切换） |
  | 通知 Notification | 340×64 | 临时消息，几秒后自动缩回，点击可关闭 |
  | 展开 Expanded | 400×190 | 鼠标悬停（有音乐时显示播放器） |
- **正在播放**（通过系统媒体传输控件 SMTC，支持 Spotify、网易云音乐、QQ 音乐、媒体播放器、浏览器视频等）
  - 播放时显示紧凑胶囊：封面、歌名、按封面主题色跳动的律动条
  - 悬停展开成播放器：封面、歌名/歌手/来源、实时进度、上一首/播放暂停/下一首
  - 暂停后胶囊继续停留 20 秒再缩回，方便马上恢复播放
  - 律动条和进度刷新只在可见时运行，空闲时不占 CPU
- 支持 Per-Monitor V2 DPI，分辨率或任务栏变化时自动重新居中
- 单实例运行；右键胶囊可以退出

## 运行

需要 Windows 10 1809 及以上，以及 [.NET 8 SDK](https://dotnet.microsoft.com/download/dotnet/8.0)。

```bash
dotnet run --project src/Island
```

也可以用 Visual Studio 2022 打开 `Island.sln`。

每次推送时 GitHub Actions 会在 Windows 上编译，并把可运行的程序作为 `Island-win-x64` 构件上传。

## 目录结构

```
src/Island/
├── App.xaml(.cs)              启动、单实例
├── app.manifest               DPI 感知声明
├── Core/IslandState.cs        状态枚举和每个状态的尺寸
├── Animation/SpringEase.cs    阻尼弹簧缓动函数
├── Interop/NativeMethods.cs   Win32：窗口样式、置顶
├── Media/
│   ├── MediaSessionService.cs SMTC 会话跟踪、读取信息、播放控制
│   ├── MediaSnapshot.cs       正在播放的信息快照（含进度推算）
│   └── ColorExtractor.cs      从封面提取主题色
└── Views/
    ├── IslandWindow.xaml(.cs) 胶囊窗口、状态机、动画
    └── IslandWindow.Media.cs  音乐相关的界面逻辑
```

## 在代码里驱动岛

```csharp
island.SetBaseState(IslandState.Compact, "正在播放：晴天");
island.ShowNotification("微信", "张三：晚上吃什么？", "💬", seconds: 4);
island.SetBaseState(IslandState.Idle);
```

## 下一步

- [x] 接入 SMTC，显示正在播放的音乐并提供播放控制
- [ ] 点击进度条跳转、换歌时短暂弹出提示
- [ ] 音量和亮度提示
- [ ] 全屏、游戏时自动隐藏
- [ ] 托盘图标、设置面板、开机自启
- [ ] Provider 模块接口和优先级调度器
