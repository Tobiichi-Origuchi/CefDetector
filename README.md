# CEF Detector [![CI](https://github.com/Tobiichi-Origuchi/CefDetectorLinux/actions/workflows/ci.yml/badge.svg)](https://github.com/Tobiichi-Origuchi/CefDetectorLinux/actions/workflows/ci.yml) [![Release](https://github.com/Tobiichi-Origuchi/CefDetectorLinux/actions/workflows/release.yml/badge.svg)](https://github.com/Tobiichi-Origuchi/CefDetectorLinux/actions/workflows/release.yml)

Check how many CEFs are on your computer

**[使用 Rust 编写，支持 Linux 和 Windows]**

> [!Note]
> 目前 Windows 支持是实验性的

看看你的电脑上有多少个 [CEF (Chromium Embedded Framework)](https://github.com/chromiumembedded/cef)

> [!Note]
> 欢迎你把程序截图发到 [Discussions](https://github.com/Tobiichi-Origuchi/CefDetectorLinux/discussions) 中, 看看谁才是真的 **《超级CEF王》**

> 你说的对，但是《LibCEF》是由谷歌自主研发的一款全新开放浏览器内核。第三方代码运行在在一个被称作「CEF」的浏览器沙盒，在这里，被前端程序员选中的代码将被授予「libcef.so」，导引浏览器之力‌。你将扮演一位名为「电脑用户」的冤种角色，在各种软件的安装中下载类型各异、体积庞大的 CEF 们，被它们一起占用硬盘空间，吃光你的内存——同时，逐步发掘「CEF」的真相

## 截屏

![Screenshot](./screenshot.webp)

## 下载

[Release](https://github.com/Tobiichi-Origuchi/CefDetectorLinux/releases)
页面提供以下构建产物：

| 平台 | 搜索后端 | 文件名 |
| --- | --- | --- |
| Linux x86_64 | ignore（并行目录遍历） | `cefdetector-<version>-linux-x86_64-ignore.tar.gz` |
| Linux x86_64 | plocate 索引 | `cefdetector-<version>-linux-x86_64-plocate.tar.gz` |
| Windows x86_64 | ignore（遍历所有逻辑盘） | `cefdetector-<version>-windows-x86_64-ignore.zip` |
| Windows x86_64 | Everything IPC | `cefdetector-<version>-windows-x86_64-everything.zip` |

Everything 后端需要预先安装并运行
[Everything](https://www.voidtools.com/)，同时启用 IPC。

> [!NOTE]
> plocate 只能搜索数据库中已有的路径，bind mount 等未被索引的目录不会被检测；它也不保证一定比默认后端更快。

## 使用

### GUI

```bash
cefdetector
```

### Cli

例如以 JSON 格式打印

```bash
cefdetector --json
```

使用 `cefdetector --help` 查看更多用法

### 忽略目录

通过创建一个配置文件来忽略特定目录

Linux 配置文件位于 `$XDG_CONFIG_HOME/cefdetector/.ignore`，Windows 配置文件位于 `%APPDATA%\cefdetector\.ignore`：

```gitignore
# 忽略目录名称（跳过所有名为 target 和 node_modules 的目录）
target
node_modules

# 忽略绝对路径
/home/user/myproject/build

# Windows 绝对路径
D:\Games\build
```

## 特性

- 检测 CEF 的类型: 如 [libcef](https://github.com/chromiumembedded/cef)、[Electron](https://www.electronjs.org/)、[NWJS](https://nwjs.io/)、[CefSharp](http://cefsharp.github.io/)、[MiniBlink](https://github.com/weolar/miniblink49)、[MiniElectron](https://github.com/weolar/miniblink49)、[Edge](https://www.microsoft.com/en-us/edge) 和 [Chrome](https://www.google.com/chrome/)
- 检测应用图标: 通过解析 PE、AppImage、同级目录、快捷方式、Linux 包管理器（APT/Pacman/RPM/Portage/Flatpak/Snap/Nix/Brew）
- 显示总空间占用
- 显示当前所运行的进程
- 单独显示每个程序的空间占用并按大小排序

## Benchmark

### Linux

一次运行 CLI 搜索、GUI 首帧启动和固定时长 GUI 采样，并将原始数据写入 CSV：

```bash
./benchmark.sh
```

只测试两种 CLI 后端并调整预热和正式运行次数：

```bash
./benchmark.sh scan --scan-warmup-runs 2 --scan-runs 10
```

只测试 GUI，并复用脚本先前保存的两个后端二进制：

```bash
./benchmark.sh gui --duration 10 --no-build --output gui-linux.csv
```

CSV 包含采样耗时、完整进程生命周期、用户态/内核态 CPU、单核与整机 CPU 占比、峰值常驻/私有内存、文件描述符数、线程数、退出状态和 CLI 结果数

### Windows

一次运行 CLI 搜索、GUI 首帧启动和固定时长 GUI 采样，并将原始数据写入 CSV：

```powershell
pwsh -NoProfile -File .\benchmark.ps1
```

只测试两种 CLI 后端并调整预热和正式运行次数：

```powershell
pwsh -NoProfile -File .\benchmark.ps1 -Mode scan -WarmupRuns 2 -ScanRuns 10
```

只测试 GUI，并复用已经构建的两个二进制：

```powershell
pwsh -NoProfile -File .\benchmark.ps1 -Mode gui -GuiDurationSeconds 10 -NoBuild
```

CSV 包含采样耗时、完整进程生命周期、用户态/内核态 CPU、整机 CPU 占比、峰值工作集、峰值私有内存、句柄数、线程数、退出状态和 CLI 结果数

## 作者

[Origuchi](https://github.com/Tobiichi-Origuchi)

创意来自 @Lakr233 的 [SafariYYDS](https://github.com/Lakr233/SafariYYDS) 及 @ShirasawaSama 的 [CefDetectorX](https://github.com/ShirasawaSama/CefDetectorX) 项目

## 协议

[MIT](./LICENSE)
