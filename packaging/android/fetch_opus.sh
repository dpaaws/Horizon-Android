#!/usr/bin/env bash
# Puts Opus 1.5.2 (xiph/opus, BSD-3-Clause) beside the voice chat engine, where its build.rs compiles it
# for Android. Only the folders the build uses are kept (about 5 MB).
#
#   packaging/android/fetch_opus.sh
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
dest=$root/native-mods/voicechat/mixins/voicechat/engine/opus
tag=v1.5.2
if [ -f "$dest/include/opus.h" ]; then
  echo "fetch_opus: already there ($dest)"
  exit 0
fi
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
git clone --quiet --depth 1 --branch "$tag" https://github.com/xiph/opus.git "$tmp/opus"
mkdir -p "$dest"
for item in include celt silk src celt_sources.mk silk_sources.mk opus_sources.mk COPYING; do
  cp -r "$tmp/opus/$item" "$dest/"
done
echo "fetch_opus: Opus $tag in $dest"
