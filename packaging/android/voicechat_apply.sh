#!/usr/bin/env bash
# Builds the voice chat into this checkout: the mod's horizon-mixins bundle (native-mods/voicechat) is
# applied to the source in place, so the normal Android build (tools/android_build.py) then compiles it
# in. Same steps as native_build.sh, up to the point where that script starts its own APK build.
# Run from the repository root, after packaging/android/fetch_opus.sh.
#
#   packaging/android/voicechat_apply.sh
set -euo pipefail

MIXINS_GIT=https://github.com/horizon-rust/horizon-mixins
# The launcher's pin (launcher/src/native_mods.rs MIXINS_REV).
MIXINS_REV=${MIXINS_REV:-5a64af938ec86b00d53d16a408aabc0038fb3e57}

fail() { echo "voicechat_apply: $*" >&2; exit 1; }
source=$(pwd)
[ -f "$source/native-mods/voicechat/mixins/voicechat/horizon-mixin.toml" ] || fail "run it from the repository root"
[ -f "$source/native-mods/voicechat/mixins/voicechat/engine/opus/include/opus.h" ] || fail "run packaging/android/fetch_opus.sh first"
[ -f "$source/.voicechat-applied" ] && fail "already applied to this checkout"

cache=${NATIVE_CACHE:-$HOME/.cache/horizon-native}
mkdir -p "$cache"
tools=$cache/mixins-${MIXINS_REV:0:12}
if [ ! -x "$tools/bin/horizon-mixins" ]; then
  cargo install --locked --git "$MIXINS_GIT" --rev "$MIXINS_REV" --root "$tools" horizon-mixins-cli
fi

commit=$(git rev-parse HEAD)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

mkdir "$work/source"
rsync -a --exclude /.git --exclude /.local --exclude /target "$source/" "$work/source/"
find "$work/source" -type l -delete
echo "$commit" > "$work/source/.horizon-revision"

manifest=crates/horizon-android/Cargo.toml
cp "$source/$manifest" "$work/android-Cargo.toml"
lib_line='path = "../horizon-client/src/android_lib.rs"'
grep -qxF "$lib_line" "$work/source/$manifest" || fail "$manifest no longer has: $lib_line"
# horizon-mixins refuses a module reached under two names: while it plans, horizon-android's library is empty.
sed -i "s|^path = \"../horizon-client/src/android_lib.rs\"\$|path = \"plan-stub.rs\"|" "$work/source/$manifest"
: > "$work/source/crates/horizon-android/plan-stub.rs"

lib=crates/horizon-client/src/lib.rs
lib_attr='#![cfg_attr(not(target_os = "android"), forbid(unsafe_code))]'
lib_attr_alt='#![forbid(unsafe_code)]'
(grep -qxF "$lib_attr" "$work/source/$lib" || grep -qxF "$lib_attr_alt" "$work/source/$lib") || fail "$lib no longer has: $lib_attr"
grep -vxF "$lib_attr" "$work/source/$lib" | grep -vxF "$lib_attr_alt" > "$work/lib.rs"
cp "$work/lib.rs" "$work/source/$lib"

mkdir -p "$work/raw_bundles" "$work/mods" "$work/api_mods" "$work/native_data" "$work/native_content"
python3 "$source/packaging/android/native_stage_bundles.py" "$work" "$commit" "$source/native-mods/voicechat"
python3 "$source/packaging/android/native_mod_adapt.py" "$work/source" "$work/mods"

cargo fetch --manifest-path "$work/source/Cargo.toml" > /dev/null
"$tools/bin/horizon-mixins" apply --workspace "$work/source" --mods "$work/mods" --out "$work/stage"

# The applied tree replaces the checkout's files (the Android library keeps its real path).
rsync -a --exclude /.git --exclude /.local --exclude /target --exclude "/crates/horizon-android/plan-stub.rs" "$work/stage/" "$source/"
cp "$work/android-Cargo.toml" "$source/$manifest"
cp "$work/stage_mods.json" "$source/.horizon-mixins-stage.json"
echo "$commit" > "$source/.voicechat-applied"
echo "voicechat_apply: voice chat applied on $commit"
