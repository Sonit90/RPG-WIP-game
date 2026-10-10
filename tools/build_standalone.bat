@echo off
rem Builds the standalone client: one mygame.exe with all content embedded and a static CRT.
rem Output: target\release\mygame.exe
setlocal
cd /d "%~dp0.."

set "RUSTFLAGS=-C target-feature=+crt-static"
cargo build --release --bin mygame --features standalone
if errorlevel 1 (
    echo.
    echo Standalone build FAILED.
    exit /b 1
)

echo.
echo Standalone client: %CD%\target\release\mygame.exe
endlocal
