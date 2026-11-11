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

# This fork's native-hooks.json lists hooks without the `version` horizon-mixins requires (they are all
# the first version): fill it in, in the work copy.
python3 - "$work/source/native-hooks.json" <<'PY'
import json, sys
path = sys.argv[1]
catalog = json.load(open(path, encoding="utf-8"))
filled = 0
for hook in catalog["hooks"]:
    if "version" not in hook:
        hook["version"] = 1
        filled += 1
json.dump(catalog, open(path, "w", encoding="utf-8"), indent=2, ensure_ascii=False)
print(f"voicechat_apply: native-hooks.json: version 1 added to {filled} hooks")
PY

mkdir -p "$work/raw_bundles" "$work/mods" "$work/api_mods" "$work/native_data" "$work/native_content"
python3 "$source/packaging/android/native_stage_bundles.py" "$work" "$commit" "$source/native-mods/voicechat"
python3 "$source/packaging/android/native_mod_adapt.py" "$work/source" "$work/mods"

cargo fetch --manifest-path "$work/source/Cargo.toml" > /dev/null
"$tools/bin/horizon-mixins" apply --workspace "$work/source" --mods "$work/mods" --out "$work/stage"

# The applied tree replaces the checkout's files (the Android library keeps its real path).
rsync -a --exclude /.git --exclude /.local --exclude /target --exclude "/crates/horizon-android/plan-stub.rs" "$work/stage/" "$source/"
cp "$work/android-Cargo.toml" "$source/$manifest"
# The Android library compiles horizon-client's own source files (android_lib.rs), so the bundle
# crates the stage added to horizon-client's dependencies must be the library's as well.
python3 - "$work/stage/crates/horizon-client/Cargo.toml" "$source/$manifest" <<'PY'
import re, sys
client = open(sys.argv[1], encoding="utf-8").read()
android = open(sys.argv[2], encoding="utf-8").read()
lines = re.findall(r'^voicechat-engine\s*=.*$', client, re.M)
if len(lines) != 1:
    sys.exit("voicechat_apply: horizon-client has no voicechat-engine dependency after the stage")
if re.search(r'^voicechat-engine\s*=', android, re.M):
    sys.exit(0)
android, n = re.subn(r'^\[dependencies\]\n', lambda m: m.group(0) + lines[0] + "\n", android, count=1, flags=re.M)
if n != 1:
    sys.exit("voicechat_apply: horizon-android has no [dependencies] table")
open(sys.argv[2], "w", encoding="utf-8").write(android)
print("voicechat_apply: horizon-android depends on", lines[0])
PY
cp "$work/stage_mods.json" "$source/.horizon-mixins-stage.json"
echo "$commit" > "$source/.voicechat-applied"
echo "voicechat_apply: voice chat applied on $commit"
