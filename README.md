# 澄音

Windows 11 本地音乐播放器。界面为简体中文。技术栈是 Tauri 2、React、TypeScript 和 Rust。

Chengyin plays local FLAC, MP3, WAV, M4A, DSF/DFF and APE, splits single-file albums with CUE sheets, and keeps the library in SQLite.

## 本地运行界面

```bash
npm install
npm run dev
```

开发服务器在 [http://127.0.0.1:1420](http://127.0.0.1:1420)。浏览器里没有 Tauri 后端时，点「添加文件夹」并确认「示例音乐」，会装入一组示例专辑，用来走查界面。

桌面窗口：

```bash
npm run tauri dev
```

Linux 上可以编译并打开窗口。DSF、DFF 和 APE 需要 Windows 安装包里的 FFmpeg。

## Windows 安装包

在 Windows 上：

```powershell
npm ci
powershell -ExecutionPolicy Bypass -File .\scripts\fetch-ffmpeg.ps1
npm run tauri build
```

`scripts/fetch-ffmpeg.ps1` 从 [BtbN/FFmpeg-Builds](https://github.com/BtbN/FFmpeg-Builds/releases) 下载 win64 GPL 静态包，放到 `src-tauri/binaries/ffmpeg-x86_64-pc-windows-msvc.exe`。二进制不入库。安装目标在 `src-tauri/tauri.windows.conf.json`：NSIS（简体中文 / English）和 MSI（zh-CN）。

GitHub Actions 工作流是 `.github/workflows/release.yml`（`windows-latest`）。推送 `v*` 标签时上传 NSIS 与 MSI，并发布到 GitHub Release。当前仓库是 Origin 草稿，这里跑不了 GitHub Actions，也发不出 v0.1.0 安装包。

安装包未签名。Windows SmartScreen 会提示未知发布者。

FFmpeg 以独立进程调用，许可是 GPL。澄音本体不静态链接 FFmpeg。

## 播放

- FLAC、MP3、WAV、M4A / ALAC：Symphonia 解码，cpal 输出。采样率不一致时用 rubato 重采样，同一采样率的相邻曲目不重置重采样器，以便无缝衔接。
- DSF / DFF：FFmpeg 先转到 176.4 kHz PCM，再重采样到设备采样率。这不是原生 DSD、DoP 或 ASIO。
- APE：FFmpeg 以文件原始采样率输出 PCM。
- CUE：按 INDEX 01 切分，75 帧/秒。编码支持 UTF-8、UTF-16，以及 GBK、Shift-JIS、GB18030、Big5。

## 测试

```bash
cargo test --lib --manifest-path src-tauri/Cargo.toml
npm run build
```
