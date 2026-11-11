#!/usr/bin/env bash
# Builds a release MyGame.app for Apple Silicon and ad-hoc signs it. Run ON a Mac.
#
#   tools/package-macos.sh [--no-overlay] [--output <dir>]
#
# Mirrors tools/package.ps1: --bin mygame with --no-default-features (the default features
# include `dev-tools`, which must never ship), only `debug-overlay` is added back by name, and
# the result is verified to contain no developer tools or test/benchmark artefacts.
set -euo pipefail

overlay=1
output=""
while [ $# -gt 0 ]; do
    case "$1" in
        --no-overlay) overlay=0 ;;
        --output) shift; output="${1:?--output needs a directory}" ;;
        -h|--help) sed -n '2,8p' "$0"; exit 0 ;;
        *) echo "error: unknown argument: $1" >&2; exit 2 ;;
    esac
    shift
done

if [ "$(uname -s)" != "Darwin" ]; then
    echo "error: package-macos.sh must run on macOS (this is $(uname -s))." >&2
    exit 1
fi
if [ "$(uname -m)" != "arm64" ]; then
    echo "error: Apple Silicon only (aarch64-apple-darwin); this machine is $(uname -m)." >&2
    exit 1
fi
if ! xcode-select -p >/dev/null 2>&1 || ! command -v xcrun >/dev/null 2>&1; then
    echo "error: Xcode command line tools are missing. Install Xcode, or run: xcode-select --install" >&2
    exit 1
fi
for tool in cargo codesign plutil rsync; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "error: required tool not found: $tool" >&2
        exit 1
    fi
done

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
output="${output:-$root/dist/macos}"
app="$output/MyGame.app"

echo "Building release binary..."
args=(build --release --bin mygame --no-default-features --target aarch64-apple-darwin)
if [ "$overlay" = 1 ]; then
    args+=(--features debug-overlay)
fi
cargo "${args[@]}"

binary="$root/target/aarch64-apple-darwin/release/mygame"
if [ ! -f "$binary" ]; then
    echo "error: expected binary not found: $binary" >&2
    exit 1
fi

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources/assets"
cp "$binary" "$app/Contents/MacOS/mygame"

# Content goes in Resources/assets, which asset_roots() searches beside the executable.
if [ -d "$root/assets" ]; then
    rsync -a --exclude 'README.md' "$root/assets/" "$app/Contents/Resources/assets/"
fi
count="$(find "$app/Contents/Resources/assets" -type f | wc -l | tr -d ' ')"
echo "Content: $count file(s)"

cat > "$app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleIdentifier</key><string>com.mygame.client</string>
    <key>CFBundleName</key><string>MyGame</string>
    <key>CFBundleDisplayName</key><string>MyGame</string>
    <key>CFBundleExecutable</key><string>mygame</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleVersion</key><string>1</string>
    <key>CFBundleShortVersionString</key><string>1.0</string>
    <key>LSMinimumSystemVersion</key><string>13.0</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>LSApplicationCategoryType</key><string>public.app-category.games</string>
</dict>
</plist>
PLIST
plutil -lint "$app/Contents/Info.plist"

echo "Verifying the package..."
fail=0
# 1. No test/benchmark artefacts outside the content.
while IFS= read -r file; do
    name="$(basename "$file")"
    case "$name" in
        *performance_test*|*cpu_bench*|*cull_bench*|*showcase*|*test*|*bench*|deps|incremental)
            echo "FAIL: test/benchmark artefact in package: $name"; fail=1 ;;
    esac
done < <(find "$app" -type f -not -path "$app/Contents/Resources/assets/*")
# 2. Exactly one executable, and it is the game.
if [ "$(find "$app/Contents/MacOS" -type f | wc -l | tr -d ' ')" != 1 ]; then
    echo "FAIL: expected exactly one executable in Contents/MacOS"; fail=1
fi
# 3. No test-only symbols, and 3b. no developer tools, linked into the binary.
for pattern in performance_test cpu_bench cull_bench engine_showcase should_panic; do
    if grep -a -q -F "$pattern" "$app/Contents/MacOS/mygame"; then
        echo "FAIL: the binary contains test/benchmark symbol: $pattern"; fail=1
    fi
done
if grep -a -q -F "mygame-dev-tools-marker" "$app/Contents/MacOS/mygame"; then
    echo "FAIL: the binary contains developer tools (the dev-tools feature was enabled)"; fail=1
fi
if [ "$fail" != 0 ]; then
    echo "error: package verification failed" >&2
    exit 1
fi

echo "Signing (ad-hoc)..."
codesign --force --deep -s - "$app"
codesign --verify --deep --strict "$app"

# 4. Smoke test: two frames prove the window, the device and the loop came up.
echo "Smoke test: running two frames..."
"$app/Contents/MacOS/mygame" --frames 2 --report 0

echo
echo "PASS: $app"
echo "Run it:    open \"$app\""
echo "With logs: \"$app/Contents/MacOS/mygame\""
echo "Settings and session live in ~/Library/Application Support/mygame."
echo "An ad-hoc signed app copied from another Mac may need:"
echo "  xattr -dr com.apple.quarantine \"$app\""
