<!-- Modified by AlanWanco for the independently maintained shiorico fork. -->
# shiorico

支持 HLS / MPEG-DASH、点播和直播的命令行下载器，由 AlanWanco 基于 [iori / Shiori](https://github.com/iori-rs/iori) 独立维护。保留原作者 Yesterday17、其他贡献者的署名、完整 Git 历史和历史 CHANGELOG。

项目仓库为 <https://github.com/AlanWanco/shiorico>，发行文件将放在该仓库的 Releases。此项目提供免费、开源版本；不是上游官方发行版。

## 下载与运行

发行包按 `shiorico-core-<target>` 和 `shiorico-ffmpeg-<target>` 命名。Windows 使用 ZIP；Linux 和 macOS 使用 tar.gz。包内包含程序、许可证、第三方声明、源码地址和构建信息。同一 Release 还应提供完整对应源码包、FFmpeg 构建配置和 `SHA256SUMS`。

`core` 不链接可选的 FFmpeg 库，仍包含 Bento4。`ffmpeg` 额外链接 FFmpeg 库。两者的 `--pipe-mux` 转播功能都会调用 PATH 中另行安装的 `ffmpeg` 命令。

```bash
shiorico --version
shiorico download 'https://example.com/playlist.m3u8' -o recording.mkv
shiorico inspect 'https://example.com/playlist.m3u8'
```

下载请求可用 `-H 'User-Agent: CustomPlayer/1.0' -H 'Referer: https://example.com/program'` 指定请求头。显式 User-Agent 优先于内置默认值，适用于播放列表和分片请求；未指定时继续使用默认 UA。

需要持续等待开播时加 `--wait`，用 `--wait-interval 60` 调整重试间隔。`--initial-segments 3` 只限制首次抓取直播列表的分片数量，用于减少启动延迟。`--no-recycle` 保留已下载分片。

直播下载默认按播放列表顺序单游标下载。加 `--dual-cursor` 后同时运行两个游标：回溯游标按时间顺序补齐播放列表窗口里的旧分片，边缘游标立刻抓取新发布的分片。回溯较慢时新分片也不会因为滚出 CDN 窗口而丢失。输出仍是一个文件，按时间顺序写入。

Eplus 活动结束且回放上线后会自动下载回放。加 `--wait` 可等待回放发布及播放列表就绪；只想下载回放时使用 `--eplus-archive`，它不会回退到直播流。回放始终从头下载，不受 `--initial-segments` 限制。

```bash
shiorico download 'https://live.eplus.jp/EVENT_ID' --eplus-archive --wait --wait-interval 60 -o archive.mkv
```

```bash
# 将 RTMP_URL 设置为自己的接收地址；不要把凭据粘贴进日志或公开报告。
shiorico download "$SOURCE_URL" --pipe-mux --initial-segments 3 \
  --output "$RTMP_URL" --no-tui --wait
```

平台登录参数见 `shiorico download --help`。自更新使用本项目的仓库、版本索引和对应构建类型：

```bash
shiorico update
shiorico update --version 0.4.0
```

`.versions/shiorico` 只在正式 Release 发布成功后记录 tag；没有已发布版本时，更新器会明确报错。

## 构建

需要稳定版 Rust、C/C++ 编译工具、CMake 和 protoc。

```bash
cargo build --release --locked -p shiorico
cargo test --locked -p shiorico --lib
```

应用源码目录暂时保留 `bin/shiori`，内部 `iori` 库、插件名和翻译资源 domain 保持兼容。主 Cargo package 和可执行文件名为 `shiorico`。目录与内部库名称不必同步重命名。

完整打包、对应源码和发布流程见 [docs/RELEASING.md](docs/RELEASING.md)。源码包包含 Cargo 依赖和 Bento4，可按其中说明离线构建。

## 项目结构

- `bin/shiori`：shiorico CLI、平台插件注册、inspect、download、update。
- `crates`：iori 下载核心、HLS / DASH、解密和可选 FFmpeg 集成。
- `plugins` 与 `platforms`：平台识别、API 和专用直播 source。
- `scripts`：固定版本 FFmpeg 构建、许可生成、打包和发行校验。
- `bin/*/CHANGELOG.md`：保留的历史变更记录。

## 许可与来源

继承的 iori 代码保留 Apache-2.0 许可及原有声明，见 [LICENSE](LICENSE) 和 [NOTICE](NOTICE)。包含 Bento4 的 shiorico 组合程序按 GPL-3.0-or-later 发行，见 [COPYING](COPYING)、[LICENSING.md](LICENSING.md) 和 [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md)。免费发行不添加“禁止商业使用”的许可限制。
