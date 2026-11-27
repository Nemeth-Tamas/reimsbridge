#!/bin/bash
set -euo pipefail

repo="$(cd "$(dirname "$0")/../.." && pwd)"
output="${1:-$repo/dist}"
mkdir -p "$output"
output="$(cd "$output" && pwd)"
version="$(awk -F '"' '/^version = "/ { print $2; exit }' "$repo/windows/Cargo.toml")"
if [[ -z "$version" ]]; then
    echo "Application version was not found." >&2
    exit 1
fi

work="$(mktemp -d "${TMPDIR:-/tmp}/reimsbridge-package.XXXXXX")"
trap 'rm -rf -- "$work"' EXIT

xcodebuild \
    -project "$repo/macos/ReimsBridge/ReimsBridge.xcodeproj" \
    -target ReimsBridge -configuration Release -sdk macosx \
    ARCHS=x86_64 ONLY_ACTIVE_ARCH=NO \
    CODE_SIGNING_ALLOWED=NO CODE_SIGNING_REQUIRED=NO CODE_SIGN_IDENTITY= \
    DEVELOPMENT_TEAM= MARKETING_VERSION="$version" \
    CONFIGURATION_BUILD_DIR="$work/build" build

app="$work/build/ReimsBridge.app"
executable="$app/Contents/MacOS/ReimsBridge"
bundle_id="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$app/Contents/Info.plist")"
if [[ "$bundle_id" != "hu.cellnet.ReimsBridge" || "$(lipo -archs "$executable")" != "x86_64" ]]; then
    echo "Unexpected bundle identifier or target architecture." >&2
    exit 1
fi
if codesign -d "$app" >/dev/null 2>&1; then
    echo "The macOS package must be unsigned." >&2
    exit 1
fi

archive="$output/ReimsBridge-macOS-x86_64.zip"
ditto -c -k --keepParent "$app" "$archive"
shasum -a 256 "$archive"
