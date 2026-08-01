# CEF Detector [![CI](https://github.com/Tobiichi-Origuchi/CefDetector/actions/workflows/ci.yml/badge.svg)](https://github.com/Tobiichi-Origuchi/CefDetector/actions/workflows/ci.yml) [![Release](https://github.com/Tobiichi-Origuchi/CefDetector/actions/workflows/release.yml/badge.svg)](https://github.com/Tobiichi-Origuchi/CefDetector/actions/workflows/release.yml)

Check how many CEFs are on your computer

看看你的电脑上有多少个 [CEF (Chromium Embedded Framework)](https://github.com/chromiumembedded/cef)

> [!Note]
> 欢迎你把程序截图发到 [Discussions](https://github.com/Tobiichi-Origuchi/CefDetector/discussions) 中, 看看谁才是真的 **《超级CEF王》**

> 你说的对，但是《LibCEF》是由谷歌自主研发的一款全新开放浏览器内核。第三方代码运行在在一个被称作「CEF」的浏览器沙盒，在这里，被前端程序员选中的代码将被授予「libcef.so」，导引浏览器之力‌。你将扮演一位名为「电脑用户」的冤种角色，在各种软件的安装中下载类型各异、体积庞大的 CEF 们，被它们一起占用硬盘空间，吃光你的内存——同时，逐步发掘「CEF」的真相

## 截屏

![Screenshot](./screenshot.webp)

## 下载

[Release](https://github.com/Tobiichi-Origuchi/CefDetector/releases)
页面提供以下构建产物：

| 平台 | 搜索后端 | 文件名 |
| --- | --- | --- |
| Linux x86_64 | ignore | `cefdetector-<version>-linux-x86_64-ignore.tar.gz` |
| Linux x86_64 | plocate | `cefdetector-<version>-linux-x86_64-plocate.tar.gz` |
| Windows x86_64 | ignore | `cefdetector-<version>-windows-x86_64-ignore.zip` |
| Windows x86_64 | Everything IPC | `cefdetector-<version>-windows-x86_64-everything.zip` |
| macOS aarch64 | ignore | `cefdetector-<version>-macos-aarch64-ignore.zip` |
| macOS aarch64 | Spotlight | `cefdetector-<version>-macos-aarch64-spotlight.zip` |

其他架构自行编译测试

### 通用

ignore 后端就是用 rust 的 ignore 库多线程枚举所有路径，速度相对慢，内存占用更高，更吃 CPU 的性能，唯一的好处是不用额外的依赖

### Windows 专用

Everything 后端需要预先安装并运行[Everything](https://www.voidtools.com/)，同时启用 IPC（精简版没有 IPC，所以不支持）

Everything 后端比 ignore 快的多

### Linux 专用

plocate 只能搜索数据库中已有的路径，bind mount 等未被索引的目录不会被检测，而且它似乎不一定比 ignore 更快

> [!NOTE]
> A bind mount is an alternate view of a directory tree. Classically, mounting creates a view of a storage device as a directory tree. A bind mount instead takes an existing directory tree and replicates it under a different point. The directories and files in the bind mount are the same as the original. Any modification on one side is immediately reflected on the other side, since the two views show the same data.
>
> 事实上 Btrfs 的一些目录就是 bind mount 的，比如 @home，所以如果你使用 Btrfs，大概率用 plocate 后端是检测不到你的家目录里的 CEF 的
>
> [这里](https://unix.stackexchange.com/questions/743060/plocate-couldnt-find-results-in-my-home-dir-but-mlocate-could-how-to-searc)有详细的讨论
>
> 解决办法在讨论中也写了：
> 1. edit `/etc/updatedb.conf`
> 2. replace `PRUNE_BIND_MOUNTS = "yes"` with `PRUNE_BIND_MOUNTS = "no"`
> 3. save the file
> 4. update the db with `sudo updatedb`

### macOS 专用

spotlight 后端就是调用 mdfind 搜索，速度比 ignore 快的多

## 使用

### GUI

```bash
cefdetector
```

### Cli

```bash
$ cefdetector --help
CEF Detector ${VERSION}

Usage: cefdetector [OPTIONS]

Options:
  -h, --help       Print help information
  -V, --version    Print version information
  -T, --toml       Output results in TOML format
  -J, --json       Output results in JSON format
  -C, --csv        Output results in CSV format
  -O, --output     Output results to the specified file path instead of stdout
```

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

这是在我的系统上实测的，共 15 个结果，6.45 GB

```plain
Summary:
  ignore, scan: elapsed 619.6 ms mean (615-625); peak RSS 61.39 MiB mean (57.07-67.82)
  ignore, gui-startup: elapsed 214.0 ms mean (214-214); peak RSS 204.77 MiB mean (204.77-204.77)
  ignore, gui: elapsed 5022.0 ms mean (5022-5022); peak RSS 251.29 MiB mean (251.29-251.29)
  plocate, scan: elapsed 591.6 ms mean (585-599); peak RSS 28.23 MiB mean (28.15-28.29)
  plocate, gui-startup: elapsed 174.0 ms mean (174-174); peak RSS 197.28 MiB mean (197.28-197.28)
  plocate, gui: elapsed 5005.0 ms mean (5005-5005); peak RSS 197.11 MiB mean (197.11-197.11)
```

### Windows

由于只有虚拟机，而且虚拟机中的 CEF 软件数量太少，测试结果没有代表性，如果有好心人愿意测试，可以：

1. 安装 rust toolchain 1.92.0-x86_64-pc-windows-msvc
2. 运行 `pwsh -NoProfile -File .\benchmark.ps1`
3. 将结果发在 issue

### macOS

由于只有虚拟机，而且虚拟机中的 CEF 软件数量太少，测试结果没有代表性，如果有好心人愿意测试，可以：

1. 安装 rust toolchain 1.92.0-aarch64-apple-darwin
2. 运行 `./benchmark_macos.sh`
3. 将结果发在 issue

## 作者

[Origuchi](https://github.com/Tobiichi-Origuchi)

创意来自 @Lakr233 的 [SafariYYDS](https://github.com/Lakr233/SafariYYDS) 及 @ShirasawaSama 的 [CefDetectorX](https://github.com/ShirasawaSama/CefDetectorX) 项目

## 协议

[MIT](./LICENSE)
