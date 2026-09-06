@echo off
chcp 65001 >nul 2>&1
title Mouse Recorder v0.5
color 0B
setlocal enabledelayedexpansion

:: Always run from this script's directory (project root)
set "BASE_DIR=%~dp0"
cd /d "%BASE_DIR%"

:: Check Python
python --version >nul 2>&1
if %ERRORLEVEL% NEQ 0 (
    echo Error: Python not found!
    echo Please install Python from: https://www.python.org/downloads/
    pause
    exit /b 1
)

:: Check if compiled binary exists (debug or release)
set EXE_PATH=
if exist "%BASE_DIR%target\release\mouse-recorder.exe" set "EXE_PATH=%BASE_DIR%target\release\mouse-recorder.exe"
if exist "%BASE_DIR%target\debug\mouse-recorder.exe" set "EXE_PATH=%BASE_DIR%target\debug\mouse-recorder.exe"
if exist "%BASE_DIR%dist\latest\mouse-recorder.exe" if not defined EXE_PATH set "EXE_PATH=%BASE_DIR%dist\latest\mouse-recorder.exe"

:menu
cls
echo =============================================
echo  Mouse Recorder - Multi-State System v0.5
echo =============================================
echo  1. Record elements (start recorder)
echo  2. Export to mouse-clicker CSV
echo  3. Verify database integrity
echo  4. List all elements
echo  5. Database statistics
echo  6. Create backup
echo  7. Build Rust program
echo  8. View help
echo  0. Exit
echo =============================================
echo.

if defined EXE_PATH (
    echo  Status: Binary found (ready to record)
) else (
    echo  Status: NOT COMPILED - select [7] to build first
)
echo.

set /p choice="Enter choice [0-8]: "

if "%choice%"=="1" goto record
if "%choice%"=="2" goto export
if "%choice%"=="3" goto verify
if "%choice%"=="4" goto list
if "%choice%"=="5" goto stats
if "%choice%"=="6" goto backup
if "%choice%"=="7" goto build
if "%choice%"=="8" goto help
if "%choice%"=="0" goto end
goto menu

:record
cls
if not defined EXE_PATH (
    echo Program not compiled.
    echo.
    set /p build_now="Build now? (y/n): "
    if /i "!build_now!"=="y" (
        cd /d "%BASE_DIR%"
        call cargo build --release
        if !ERRORLEVEL! NEQ 0 (
            echo Build failed.
            pause
            goto menu
        )
        set "EXE_PATH=%BASE_DIR%target\release\mouse-recorder.exe"
        echo Build successful!
    ) else (
        pause
        goto menu
    )
)

echo Starting recorder...
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
"%EXE_PATH%"
echo.
pause
goto menu

:export
cls
echo Export to mouse-clicker CSV
echo.
set /p export_file="Enter export filename (without extension): "

if "%export_file%"=="" set export_file=export

cd /d "%BASE_DIR%"
python scripts\safe_export.py "%export_file%.csv"
echo.
pause
goto menu

:verify
cls
echo Verifying database integrity...
echo.
cd /d "%BASE_DIR%"
python scripts\db_manager.py verify
echo.
pause
goto menu

:list
cls
echo Listing all elements...
echo.
cd /d "%BASE_DIR%"
python scripts\db_manager.py list
echo.
pause
goto menu

:stats
cls
echo Database statistics...
echo.
cd /d "%BASE_DIR%"
python scripts\db_manager.py stats
echo.
pause
goto menu

:backup
cls
echo Creating database backup...
echo.
cd /d "%BASE_DIR%"
python scripts\db_manager.py backup
echo.
pause
goto menu

:build
cls
echo ============================================
echo  Building Rust Program
echo ============================================
echo.
echo  Make sure you have Rust installed:
echo    https://rustup.rs/
echo.
echo  Two options:
echo    [1] Quick build (debug, faster)
echo    [2] Release build (optimized, slower)
echo    [3] Cancel
echo.

set /p build_choice="Choose [1/2/3]: "

cd /d "%BASE_DIR%"

if "%build_choice%"=="1" (
    echo Building (debug mode)...
    call cargo build
    if %ERRORLEVEL%==0 (
        echo Build successful!
        set "EXE_PATH=%BASE_DIR%target\debug\mouse-recorder.exe"
    ) else (
        echo Build failed.
    )
)
if "%build_choice%"=="2" (
    echo Building (release mode)...
    call cargo build --release
    if %ERRORLEVEL%==0 (
        echo Build successful!
        set "EXE_PATH=%BASE_DIR%target\release\mouse-recorder.exe"
        copy /Y "%EXE_PATH%" "%BASE_DIR%dist\latest\mouse-recorder.exe" >nul
    ) else (
        echo Build failed.
    )
)
echo.
pause
goto menu

:help
cls
echo ============================================
echo  Help - Mouse Recorder System
echo ============================================
echo.
echo Files:
echo  config.toml             Configuration file
echo  scripts\manage.py       Python management tool
echo  scripts\db_manager.py   Database manager
echo  scripts\safe_export.py  Safe export tool
echo  start.bat               This launcher
echo  record.bat              Quick record launcher
echo.
echo Documents:
echo  README.md               Project overview
echo  docs\QUICKSTART_V3.md   Quick start guide
echo  docs\MULTI_STATE_GUIDE.md  System guide
echo.
echo Workflow:
echo  1. [1] Record - capture elements with multiple states
echo  2. [2] Export - generate CSV for mouse-clicker
echo  3. Load CSV in mouse-clicker and run
echo.
pause
goto menu

:end
cls
echo.
echo Thanks for using Mouse Recorder System v0.5
echo.
timeout /t 2 >nul
exit /b 0
