# Licensing

shiorico is a free, open-source continuation of [iori / Shiori](https://github.com/iori-rs/iori), independently maintained by AlanWanco. It is available without a purchase requirement. The licenses below also permit commercial use; this project does not add a noncommercial restriction.

The inherited iori libraries, plugins, and source retain their Apache-2.0 license and author notices. The original license text remains unchanged in `LICENSE`. Fork modifications are recorded in Git history and in modification notices in affected files. Historical changelogs and upstream attribution are retained.

The **combined shiorico application** is distributed under **GPL-3.0-or-later**, with the license text in `COPYING`. Both the `core` and `ffmpeg` editions include Bento4 through `iori -> dash-mpd -> mp4decrypt -> bento4-src`. Bento4's C++ source headers permit GPL-2.0-or-later, so GPLv3 is used for this combined distribution alongside Apache-2.0 components. The Rust wrapper metadata alone does not describe the license of the bundled C++ code. This distribution uses Bento4's GPL option, without relying on a separate commercial agreement.

Individual components retain their own licenses. Applying GPLv3 to the combined distribution does not replace Apache-2.0, MIT, BSD, LGPL, or other component notices. See `THIRD-PARTY-NOTICES.md`, `LICENSES/Bento4.txt`, and the generated `THIRD-PARTY-LICENSES.txt` in each binary package.

The `ffmpeg` edition additionally links the pinned FFmpeg source in `.versions/ffmpeg`. Release builds disable library autodetection, do not enable nonfree libraries, and retain the actual configuration and full source. FFmpeg's applicable license texts are included with its source; configuration and revision are recorded for each target. Invoking an independently installed `ffmpeg` process for `--pipe-mux` is separate from this optional linked library feature.

Each published version must include the corresponding source archive, vendored Cargo dependencies with Bento4 C++ source, FFmpeg source when linked, per-target build configuration, and checksums. The source archive, binary archives, and their `SOURCE.txt` / `build-info.json` must refer to the same version and revision. Release automation validates the expected materials before publication. See `docs/RELEASING.md` for the build and release procedure.

This document describes this repository's distribution choices; it is not a replacement for the license texts or a promise about third-party services.
