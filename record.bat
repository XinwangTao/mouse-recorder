@echo off
chcp 65001 >nul 2>&1
title Mouse Recorder - Record Mode
color 0A

set BASE_DIR=D:\mouse-recorder

:: Find binary
set EXE_PATH=
if exist "%BASE_DIR%\target\release\mouse-recorder.exe" set EXE_PATH=%BASE_DIR%\target\release\mouse-recorder.exe
if exist "%BASE_DIR%\target\debug\mouse-recorder.exe" set EXE_PATH=%BASE_DIR%\target\debug\mouse-recorder.exe

if not defined EXE_PATH (
    echo ==========================================
    echo  Mouse Recorder - NOT COMPILED
    echo ==========================================
    echo.
    echo Binary not found. Build first:
    echo   cd /d %BASE_DIR%
    echo   cargo build --release
    echo.
    pause
    exit /b 1
)

echo ==========================================
echo  Mouse Recorder - Recording Mode
echo ==========================================
echo.
echo Controls:
echo  [N] New element (select region - name - state)
echo  [A] Add state to current element
echo  [C] Cancel selection
echo  [L] List all elements
echo  [E] Export to CSV
echo  [Esc] Exit
echo.
echo Workflow:
echo  1. Press N, move mouse to element, press N
echo  2. Enter element name and state name
echo  3. Press A to add more states
echo  4. Press E to export for mouse-clicker
echo.
pause

cd /d "%BASE_DIR%"
start "" "%EXE_PATH%"
