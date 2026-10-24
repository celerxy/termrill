#!/usr/bin/env bash
# 校验 Tauri 已签名的 .app 和 DMG 内的副本；不在打包后补签，避免镜像仍含旧应用。
set -euo pipefail

if [[ "$(uname -s)" != Darwin || $# -ne 2 ]]; then
  echo 'Usage on macOS: verify-macos-signature.sh <release/bundle> <Rust target>' >&2
  exit 1
fi
case "$2" in
  x86_64-apple-darwin) architecture=x86_64 ;;
  aarch64-apple-darwin) architecture=arm64 ;;
  *) echo 'Unsupported macOS architecture.' >&2; exit 1 ;;
esac
bundle_root="$(cd "$1" && pwd -P)"
shopt -s nullglob
images=("$bundle_root"/dmg/*.dmg)
if [[ ${#images[@]} -ne 1 ]]; then
  echo 'Expected exactly one newly built DMG.' >&2
  exit 1
fi

verify_app() {
  local app="$1" details uuid executable
  # Bash 命令替换内不能依赖 set -e；每项失败都显式返回，防止哈希命令掩盖签名错误。
  [[ -d "$app" && ! -L "$app" ]] || return 1
  /usr/bin/codesign --verify --deep --strict --verbose=2 "$app" || return 1
  details="$(/usr/bin/codesign -dv --verbose=4 "$app" 2>&1)" || return 1
  printf '%s\n' "$details" | grep -Fxq 'Signature=adhoc' || return 1
  printf '%s\n' "$details" | grep -Fxq 'Identifier=dev.termrill.sftpclient' || return 1
  executable="$(/usr/bin/plutil -extract CFBundleExecutable raw -o - "$app/Contents/Info.plist")" || return 1
  [[ "$executable" == termrill-sftp-client ]] || return 1
  uuid="$(/usr/bin/dwarfdump --uuid "$app/Contents/MacOS/$executable")" || return 1
  [[ "$(printf '%s\n' "$uuid" | wc -l | tr -d ' ')" == 1 ]] || return 1
  printf '%s\n' "$uuid" | grep -Eq "^UUID: [[:xdigit:]]{8}-[[:xdigit:]]{4}-[[:xdigit:]]{4}-[[:xdigit:]]{4}-[[:xdigit:]]{12} \($architecture\) " || return 1
  if printf '%s\n' "$uuid" | grep -q '^UUID: 00000000-0000-0000-0000-000000000000 '; then
    echo 'Main executable UUID must not be empty.' >&2
    return 1
  fi
  /usr/bin/shasum -a 256 "$app/Contents/MacOS/$executable" | awk '{print $1}'
}

source_hash="$(verify_app "$bundle_root/macos/TermRill.app")"
temporary="$(mktemp -d "${TMPDIR:-/tmp}/termrill-signature.XXXXXX")"
mountpoint="$temporary/volume"
mkdir "$mountpoint"
mounted=0
cleanup() {
  local status=$?
  trap - EXIT
  if [[ $mounted == 1 ]]; then
    /usr/bin/hdiutil detach "$mountpoint" -quiet || status=1
  fi
  # 只移除本脚本创建的空目录；卸载失败时不递归删除挂载内容。
  rmdir "$mountpoint" "$temporary" || status=1
  exit "$status"
}
trap cleanup EXIT
/usr/bin/hdiutil attach "${images[0]}" -readonly -nobrowse -mountpoint "$mountpoint" -quiet
mounted=1
image_hash="$(verify_app "$mountpoint/TermRill.app")"
[[ "$source_hash" == "$image_hash" ]]
echo "Verified ad-hoc signature, executable UUID and DMG contents for $architecture."
