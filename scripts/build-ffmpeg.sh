#!/usr/bin/env bash
# shiorico release build: keep the complete, pinned FFmpeg source and configuration.
set -euo pipefail

release_target=${1:?Usage: build-ffmpeg.sh TARGET [SOURCE_ROOT]}
release_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
source_root=${2:-"$release_root/target/native/$release_target"}
mkdir -p "$source_root"
source_root=$(cd "$source_root" && pwd)
ffmpeg_revision=$(tr -d '[:space:]' < "$release_root/.versions/ffmpeg")
ffmpeg_source="$source_root/ffmpeg"
ffmpeg_prefix="$source_root/ffmpeg-install"

if [[ ! -d "$ffmpeg_source/.git" ]]; then
  git init "$ffmpeg_source"
  git -C "$ffmpeg_source" remote add origin https://github.com/FFmpeg/FFmpeg.git
fi
git -C "$ffmpeg_source" fetch --depth=1 origin "$ffmpeg_revision"
git -C "$ffmpeg_source" checkout --detach FETCH_HEAD
test "$(git -C "$ffmpeg_source" rev-parse HEAD)" = "$ffmpeg_revision"

configure=(
  "--prefix=$ffmpeg_prefix"
  --disable-decoder=exr,phm --disable-programs --disable-doc
  --disable-autodetect --enable-static --disable-shared --enable-pic
)
case "$release_target" in
  x86_64-pc-windows-gnu)
    configure+=(--arch=x86_64 --target-os=mingw32 --cross-prefix=x86_64-w64-mingw32-
      --enable-cross-compile --pkg-config=pkg-config --pkg-config-flags=--static
      --extra-libs=-lstdc++ '--extra-cflags=-static -static-libgcc'
      '--extra-cxxflags=-static -static-libgcc -static-libstdc++'
      '--extra-ldflags=-static -static-libgcc -static-libstdc++')
    ;;
  x86_64-apple-darwin)
    configure+=(--arch=x86_64 --target-os=darwin --enable-cross-compile --cc=clang --cxx=clang++
      '--extra-cflags=-arch x86_64' '--extra-cxxflags=-arch x86_64' '--extra-ldflags=-arch x86_64')
    ;;
  aarch64-apple-darwin|x86_64-unknown-linux-gnu|aarch64-unknown-linux-gnu) ;;
  *) printf 'Unsupported target: %s\n' "$release_target" >&2; exit 1 ;;
esac

cd "$ffmpeg_source"
./configure "${configure[@]}"
make -j "$(getconf _NPROCESSORS_ONLN)"
make install
mkdir -p "$source_root/ffmpeg-build-info/licenses"
cp ffbuild/config.mak config.h "$source_root/ffmpeg-build-info/"
cp LICENSE.md COPYING.* "$source_root/ffmpeg-build-info/licenses/"
printf '%q ' ./configure "${configure[@]}" > "$source_root/ffmpeg-build-info/configure-command.txt"
printf '\n' >> "$source_root/ffmpeg-build-info/configure-command.txt"
printf '%s\n' "$ffmpeg_revision" > "$source_root/ffmpeg-build-info/SOURCE_REVISION"

if [[ -n "${GITHUB_ENV:-}" ]]; then
  {
    printf 'FFMPEG_INCLUDE_DIR=%s/include\n' "$ffmpeg_prefix"
    printf 'FFMPEG_PKG_CONFIG_PATH=%s/lib/pkgconfig\n' "$ffmpeg_prefix"
    printf 'PKG_CONFIG_PATH_FOR_TARGET=%s/lib/pkgconfig\n' "$ffmpeg_prefix"
    printf 'PKG_CONFIG_ALLOW_CROSS=1\n'
    printf 'SHIORICO_FFMPEG_INFO=%s/ffmpeg-build-info\n' "$source_root"
    if [[ "$release_target" == x86_64-pc-windows-gnu ]]; then
      printf 'FFMPEG_LIBS_DIR=%s/lib\n' "$ffmpeg_prefix"
    fi
  } >> "$GITHUB_ENV"
fi
printf 'Built FFmpeg %s for %s in %s\n' "$ffmpeg_revision" "$release_target" "$ffmpeg_prefix"
