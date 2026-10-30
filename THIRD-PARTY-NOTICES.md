# Third-party notices

shiorico is derived from [iori / Shiori](https://github.com/iori-rs/iori). Its original Apache-2.0 license, contributor history, and historical changelogs remain part of this source distribution. See `LICENSE` and `NOTICE`.

## Bento4

Bento4 is developed by Axiomatic Systems, LLC and contributors. Website: <https://www.bento4.com/>. The dependency route is `iori -> dash-mpd -> mp4decrypt -> bento4-src`. `Cargo.lock` pins the Rust wrapper and its bundled C++ source. The bundled Bento4 code is offered under GPL-2.0-or-later or a commercial license; this project uses the GPL option and distributes the combined application under GPL-3.0-or-later. The wrapper's MIT/Apache metadata must not be interpreted as relicensing its bundled C++ code. The upstream Bento4 license statement is retained in `LICENSES/Bento4.txt`, and complete bundled source is included in release source archives.

## FFmpeg

FFmpeg is included only in the linked `ffmpeg` edition. Website: <https://ffmpeg.org/>. The revision is pinned in `.versions/ffmpeg`; source and license documents are included under `native/ffmpeg` in the release source archive. Per-target binary archives and separate build-info archives retain `configure-command.txt`, `config.h`, `config.mak`, and FFmpeg's license texts under `ffmpeg-build-info/licenses`. Release builds disable autodetection of external libraries and do not enable nonfree libraries.

## Rust dependencies

`about.toml` defines the accepted component license expressions for this GPLv3 distribution. `cargo-about 0.9.3` generates each binary package's `THIRD-PARTY-LICENSES.txt` from the locked graph for that target and feature set, retaining identified license and copyright text. The report describes Rust packages; the additional native Bento4 and FFmpeg notices above remain necessary. Vendored dependency source includes original license files and bundled native sources.

Generated material must be refreshed when dependencies, features, or targets change. `scripts/package-release.py` refuses packaging when license generation fails; `scripts/validate-release.py` checks the release's required files and matching source revision before publication.
