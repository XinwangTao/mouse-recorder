@echo off
chcp 65001 >nul 2>&1
title Mouse Recorder - Record Mode
color 0A

set "BASE_DIR=%~dp0"
cd /d "%BASE_DIR%"

:: Find binary (prefer release build, then dist/latest)
set EXE_PATH=
if exist "%BASE_DIR%target\release\mouse-recorder.exe" set "EXE_PATH=%BASE_DIR%target\release\mouse-recorder.exe"
if not defined EXE_PATH if exist "%BASE_DIR%dist\latest\mouse-recorder.exe" set "EXE_PATH=%BASE_DIR%dist\latest\mouse-recorder.exe"
if not defined EXE_PATH if exist "%BASE_DIR%target\debug\mouse-recorder.exe" set "EXE_PATH=%BASE_DIR%target\debug\mouse-recorder.exe"

if not defined EXE_PATH (
    echo ==========================================
    echo  Mouse Recorder - NOT COMPILED
    echo ==========================================
    echo.
    echo Binary not found. Build first:
    echo   cd /d "%BASE_DIR%"
    echo   cargo build --release
    echo.
    echo Or run: start.bat  then choose [7] Build
    echo.
    pause
    exit /b 1
)

echo ==========================================
echo  Mouse Recorder - Recording Mode
echo ==========================================
echo.
echo Controls:
echo  [F5] New element
echo  [F6] Add state
echo  [F11] Verify match
echo  [F12] Re-record
echo  [Esc] Exit
echo.
pause

cd /d "%BASE_DIR%"
start "" "%EXE_PATH%"
