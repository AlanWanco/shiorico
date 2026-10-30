# shiorico 构建与发行

源码目录保留 `bin/shiori`，Cargo package 和 executable 为 `shiorico`。继承的 iori 库和插件保持原名。主应用版本取自 `bin/shiori/Cargo.toml`，正式 tag 格式为 `shiorico-v0.4.0`，更新索引为 `.versions/shiorico`。

## 工具与本机构建

需要稳定版 Rust、Python 3.12+、CMake、C/C++ 编译工具、protoc；生成许可报告需 `cargo-about 0.9.3`。链接 FFmpeg 时还需要 NASM 和 pkg-config。Windows 交叉构建需要 MinGW-w64；macOS x86_64 使用 Clang 的 `-arch x86_64`。

```bash
cargo install cargo-about --version 0.9.3 --locked
cargo fetch --locked
cargo test --locked -p shiorico --lib
cargo build --release --locked -p shiorico --target aarch64-apple-darwin
python3 scripts/package-release.py binary --version 0.4.0 \
  --variant core --target aarch64-apple-darwin
```

版本未提交时，打包信息会标记 `-dirty`，发行校验会拒绝发布。`SHIORICO_BUILD_COMMIT` 在 CI 中设为完整 commit；源码包以 `SOURCE_REVISION` 保存相同标识。不为未提交状态生成虚假的正式版本索引。

## Eplus 本地与公开构建

公开源码默认在 Eplus `PREPARING`（准备中）时返回无可下载源；加 `--wait` 时沿用现有等待间隔重试，不开始下载分片。公开 Actions 对 core 和 FFmpeg 版均固定 `SHIORICO_LOCAL_EPLUS=0`，发行校验拒绝本地策略。

如需本地构建在准备中仍探测可访问的播放列表，在仓库根目录创建 `.shiorico-local-eplus`，再正常执行上述 `cargo build`。该标记已被 Git 忽略，不进入公开仓库或源码包；本机已编译程序的行为不会随标记变化，需重新编译。也可在构建命令前显式设置 `SHIORICO_LOCAL_EPLUS=1`；`0` 强制公开策略并覆盖本地标记。环境变量在编译时读取，运行时不提供绕过参数。

本地版的 `--version` 增加 `local-eplus` 标识，打包信息记录 `eplus_preparing_policy: probe`；公开包记录 `stop`。本地版仍要求播放列表有效，准备中不会保证已经有可用分片。其他 Eplus 状态与选流流程保持既有行为。

## FFmpeg 版

`.versions/ffmpeg` 固定完整 FFmpeg commit，目前为 FFmpeg n7.1.3 对应提交。`scripts/build-ffmpeg.sh` 从固定提交构建，禁用外部库自动检测，保存实际 `configure` 参数、`config.h`、`ffbuild/config.mak` 及 FFmpeg 许可正文。不依赖浮动 release branch。

```bash
bash scripts/build-ffmpeg.sh aarch64-apple-darwin
export FFMPEG_INCLUDE_DIR="$PWD/target/native/aarch64-apple-darwin/ffmpeg-install/include"
export FFMPEG_PKG_CONFIG_PATH="$PWD/target/native/aarch64-apple-darwin/ffmpeg-install/lib/pkgconfig"
export PKG_CONFIG_PATH_FOR_TARGET="$FFMPEG_PKG_CONFIG_PATH"
export PKG_CONFIG_ALLOW_CROSS=1
cargo build --release --locked -p shiorico --features ffmpeg --target aarch64-apple-darwin
python3 scripts/package-release.py binary --version 0.4.0 \
  --variant ffmpeg --target aarch64-apple-darwin \
  --ffmpeg-info target/native/aarch64-apple-darwin/ffmpeg-build-info
```

Windows 另设置 `FFMPEG_LIBS_DIR` 为该目标 install 下的 `lib`，由 `.cargo/config.toml` 和 `scripts/linker-wrapper.sh` 链接。Build workflow 会自动设置这些变量。

## 完整对应源码

```bash
cargo fetch --locked
python3 scripts/package-release.py source --version 0.4.0 \
  --ffmpeg-source target/native/aarch64-apple-darwin/ffmpeg
```

只发行 core 时可省略 `--ffmpeg-source`。脚本将项目源码、`Cargo.lock`、所有 vendor 依赖、Bento4 C++、许可文本和构建脚本打包。FFmpeg 版额外包含 `native/ffmpeg` 的固定完整源码。各目标实际构建配置在对应二进制包中，并另附 `*-build-info.tar.gz`。源文件归档不包含 `.git`、`target` 或本地凭据。

从源码包编译：先安装上述工具，然后运行 `cargo build --release --locked --offline -p shiorico --target <target>`。源码包的 `.cargo/config.toml` 已加入 vendor 替换；`scripts/linker-wrapper.sh` 保留 Windows 链接配置。FFmpeg 版需先使用包内 `native/ffmpeg` 和发布的该目标 `configure-command.txt` 参数重建：将 `--prefix` 改为本机 install 目录；保留其他参数，再执行 `make` / `make install`。随后按上节设置 FFmpeg 环境变量并执行 Cargo `--features ffmpeg` 构建。

## Actions

`Build` 中 5 个目标 × core/ffmpeg 各自完成 FFmpeg 构建、应用构建和打包。`fail-fast: false`，任何目标失败都不会阻止其他目标生成 artifact。Windows 的步骤不依赖 macOS/Linux job。

手动触发 `Release`，默认创建 draft；`core_only` 可只构建 core。发布流程调用相同构建 workflow，另行制作完整源码。最后检查所选目标的所有压缩包、许可材料、源码 revision 和 SHA256，一致后才创建 draft 并上传。

`publish: true` 会在上传完成且资产列表验证通过后公开 Release，随后才向当前分支提交 `.versions/shiorico` 索引更新。这个选项明确授权 workflow 创建 tag、发布及提交索引。手动仅检查构建时保留 `publish: false`；流程不会更新索引。仓库需允许 `GITHUB_TOKEN` 写入内容并向分支推送，否则最后的索引更新会失败，需要按分支策略处理。

正式发布时不要复用已有 tag。若某次 draft 流程中断，先检查其已有资产与 revision，再决定清理或另开版本；脚本不会自动覆盖已存在的 release。

## 校验与产物

```bash
python3 scripts/validate-release.py dist/release --version 0.4.0
# 仅检查一次 core/macOS ARM64 发行：
python3 scripts/validate-release.py dist/release --version 0.4.0 \
  --variants core --targets aarch64-apple-darwin
```

每个二进制包必须包含主程序、README、Apache 与 GPL 正文、NOTICE、许可关系、第三方报告、Bento4 声明、源码链接及 `build-info.json`。FFmpeg 版还包含原生构建配置及 `ffmpeg-build-info/licenses` 下的 FFmpeg 许可正文。Release 同时上传完整源码和 `SHA256SUMS`。GitHub 自动生成的 Source code ZIP/tar.gz 不能代替完整对应源码包。

上传前检查：版本号和 tag、对应源码与二进制 commit、全部资产和校验值；Windows PE 架构检查只能证明格式，仍需 Windows 主机运行验证。Actions 结果与真实平台下载、转播行为分开记录。
