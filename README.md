# Island

Windows 上的"灵动岛"：屏幕顶部正中的一个黑色胶囊。鼠标移上去展开成功能面板，移开立刻收起。

WPF + .NET 8，轻量、开箱即用。

## 下载

到 [Releases](https://github.com/MoonFlower-King/island/releases) 下载最新版本，解压后双击 `Island.exe` 即可，不需要安装。

- `win-x64`：大多数电脑（Intel / AMD）
- `win-arm64`：ARM 处理器的电脑（如骁龙笔记本）
- `win-x64-framework-dependent`：已安装 .NET 8 桌面运行时的电脑，体积更小

系统要求：Windows 10 1809 或更高版本、Windows 11。想开机自动运行，右键胶囊勾选"开机自动启动"。

## 怎么用

- **展开**：把鼠标移到屏幕顶部正中的胶囊上（甩到屏幕最顶端也可以）。
- **收起**：鼠标离开就会收起。正在打字时不会因为鼠标移开而收起，按 `Esc` 或点击别处即可。
- **切换功能**：展开后点顶部的图标。下次展开时停在上次用的功能。
- **右键胶囊**：开机自动启动、打开数据文件夹、开关剪贴板历史保存、退出。
- **全屏时自动隐藏**：看视频、打游戏、放 PPT 时岛会自动让开。

## 功能

| 图标 | 功能 | 说明 |
|---|---|---|
| 🎵 | 音乐 | 读取系统媒体控制（SMTC），支持 Spotify、网易云音乐、QQ 音乐、媒体播放器、浏览器视频等。播放时胶囊显示封面、歌名和随封面变色的律动条；展开后可以上一首、播放/暂停、下一首，并显示进度。从音乐胶囊展开时会直接打开播放器。 |
| ✅ | 日常待办 | 回车添加，勾选完成后自动沉底，一键清除已完成。 |
| 📋 | 剪贴板历史 | 自动记录最近 50 条复制的文字，点一下重新复制。密码管理器标记为"不记录"的内容会被跳过。 |
| 📁 | 文件中转站 | 把文件拖到岛上暂存，之后从岛上拖到别处使用。双击打开，可以在资源管理器中定位。只记录路径，不复制文件。 |
| 🌐 | AI 翻译 | 使用 Claude 翻译，边生成边显示。中文翻成英文，其他语言翻成中文。可以一键粘贴剪贴板并翻译。 |
| 📊 | 系统监控 | CPU、内存占用和实时上传/下载网速。 |

### 轻量

- 只在看得见的时候干活：律动条、播放进度、系统监控都只在对应界面显示时运行，收起后不占 CPU。
- 时钟每分钟只刷新一次。
- 剪贴板通过系统通知（`AddClipboardFormatListener`）获取变化，不轮询。
- 不点击岛就不会抢走焦点，也不会出现在任务栏和 Alt+Tab 里。

## AI 翻译的设置

翻译需要一个 [Anthropic API Key](https://console.anthropic.com/)。第一次打开翻译标签时会提示填写，有两种方式：

- 直接在面板里粘贴 Key。Key 会用 Windows DPAPI 加密后保存在本机，只有当前 Windows 用户能解密。
- 设置环境变量 `ANTHROPIC_API_KEY`。设置后优先使用环境变量。

默认使用 `claude-opus-5-5`，并把 effort 设为 low，让翻译更快、更省。如果想换模型，修改 `settings.json` 里的 `TranslateModel`。

## 数据保存在哪

全部保存在 `%APPDATA%\Island\`，右键胶囊选择"打开数据文件夹"就能看到：

| 文件 | 内容 |
|---|---|
| `settings.json` | 上次的标签、翻译模型、加密的 API Key、剪贴板保存开关 |
| `todos.json` | 待办 |
| `clipboard.json` | 剪贴板历史（可以在右键菜单里关闭保存，关闭时会删除这个文件） |
| `shelf.json` | 中转站里的文件路径 |
| `island.log` | 运行日志。遇到问题时可以附上这个文件 |

## 开发

需要 Windows 10 1809 及以上，以及 [.NET 8 SDK](https://dotnet.microsoft.com/download/dotnet/8.0)。

```bash
dotnet run --project src/Island
```

```bash
dotnet test
```

也可以用 Visual Studio 2022 打开 `Island.sln`。

### 冒烟测试

`Island.exe --smoke-test <输出目录>` 会在真实窗口上走一遍所有状态和功能标签，检查状态切换和尺寸，并把每个界面截图保存成 PNG。结果写到 `report.txt`，全部通过时退出码为 0。设置环境变量 `ISLAND_DATA_DIR` 可以把数据目录指到别处，避免影响自己的数据。

### CI 与发布

每次推送，GitHub Actions 都会在 Windows 上依次编译、运行单元测试、发布单文件程序，并对发布出来的 exe 运行冒烟测试。截图作为 `smoke-test` 构件上传。

发布新版本：

1. 在 `CHANGELOG.md` 里写好新版本的说明（标题格式 `## [1.2.3]`）
2. 推送对应的标签

```bash
git tag v1.2.3
```

```bash
git push origin v1.2.3
```

CI 会用标签里的版本号构建，并自动创建 GitHub Release，附上 x64、arm64 和依赖运行时的三个压缩包，以及 SHA256 校验文件。

## 目录结构

```
src/Island/
├── App.xaml(.cs)                 启动、单实例、全局样式
├── Core/                         状态与尺寸、设置、JSON 存储
├── Animation/SpringEase.cs       阻尼弹簧缓动
├── Interop/NativeMethods.cs      Win32：窗口样式、置顶、剪贴板监听、光标位置
├── Media/                        SMTC 媒体会话、封面取色
├── Services/                     待办、剪贴板、中转站、翻译、系统监控
└── Views/
    ├── IslandWindow.xaml(.cs)    胶囊窗口、状态机、悬停展开/收起、全屏隐藏
    ├── IslandWindow.Features.cs  功能标签、键盘输入、拖放、剪贴板监听
    ├── IslandWindow.Media.cs     音乐胶囊与播放器
    ├── IslandWindow.SmokeTest.cs 冒烟测试
    └── Panels/                   待办、剪贴板、中转站、翻译、监控面板
tests/Island.Tests/               单元测试
```
