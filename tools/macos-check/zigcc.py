"""C compiler shim for type-checking the macOS target from Windows.

Build scripts that compile C (ring, blake3) need a compiler for aarch64-apple-darwin; zig
provides one. The cc crate passes its own --target / -arch / -mmacosx-version-min flags in
Apple-clang spelling, which zig rejects, so they are dropped and zig's own target is used.
Never used to produce a shipped binary: macOS builds happen on a Mac with Xcode.
"""
import subprocess, sys

args = []
skip = False
for a in sys.argv[1:]:
    if skip:
        skip = False
        continue
    if a.startswith("--target=") or a.startswith("-mmacosx-version-min") or a.startswith("-mmacos-version-min"):
        continue
    if a in ("-arch", "-target", "--target", "-isysroot"):
        skip = True
        continue
    args.append(a)
sys.exit(subprocess.call([sys.executable, "-m", "ziglang", "cc", "-target", "aarch64-macos", *args]))
