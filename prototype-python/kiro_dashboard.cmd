@echo off
REM ==========================================================
REM  Kiro Usage Dashboard - Windows launcher
REM  Double-click, or pass CLI args, e.g.
REM     kiro_dashboard.cmd --port 9000
REM     kiro_dashboard.cmd --host 0.0.0.0 --no-browser
REM
REM  Notes:
REM  - This file MUST be saved with CRLF line endings and use only
REM    ASCII characters in comments/code. Otherwise cmd.exe will
REM    mis-parse it on zh-CN Windows.
REM  - Python auto-detected in this order:
REM      1) KIRO_PYTHON env var (full path to python.exe)
REM      2) common miniconda / anaconda install locations
REM      3) "py" launcher (from python.org)
REM      4) plain "python" on PATH (may hit MS Store stub!)
REM ==========================================================

setlocal EnableExtensions

set "PYTHONIOENCODING=utf-8"
set "PYTHONUTF8=1"

cd /d "%~dp0"

REM ---------------- locate python ----------------
set "PYEXE="
set "PYCMD="

REM 1) explicit override
if defined KIRO_PYTHON if exist "%KIRO_PYTHON%" set "PYEXE=%KIRO_PYTHON%"
if defined PYEXE goto :got_python

REM 2) common conda absolute paths (no PATH lookup -> dodges Store stub)
call :try_path "%USERPROFILE%\AppData\Local\miniconda3\python.exe"
if defined PYEXE goto :got_python
call :try_path "%USERPROFILE%\AppData\Local\anaconda3\python.exe"
if defined PYEXE goto :got_python
call :try_path "%USERPROFILE%\miniconda3\python.exe"
if defined PYEXE goto :got_python
call :try_path "%USERPROFILE%\anaconda3\python.exe"
if defined PYEXE goto :got_python
call :try_path "C:\ProgramData\miniconda3\python.exe"
if defined PYEXE goto :got_python
call :try_path "C:\ProgramData\anaconda3\python.exe"
if defined PYEXE goto :got_python
call :try_path "C:\miniconda3\python.exe"
if defined PYEXE goto :got_python
call :try_path "C:\anaconda3\python.exe"
if defined PYEXE goto :got_python

REM 3) py launcher
where py >nul 2>nul
if not errorlevel 1 (
    set "PYCMD=py"
    goto :got_python
)

REM 4) plain python on PATH  (last resort; may be Store stub)
where python >nul 2>nul
if not errorlevel 1 (
    set "PYCMD=python"
    goto :got_python
)

REM ---- nothing found ----
echo.
echo [ERROR] No Python 3 interpreter found.
echo.
echo Fix options:
echo   1. Install Python 3.9+ from python.org (tick "Add to PATH")
echo   2. Or set an env var pointing to your python.exe:
echo         setx KIRO_PYTHON "C:\path\to\python.exe"
echo      then open a NEW terminal and retry.
echo.
pause
exit /b 1

:try_path
if exist "%~1" set "PYEXE=%~1"
goto :eof

:got_python

REM ---------------- run ----------------
if defined PYEXE (
    echo Using Python: "%PYEXE%"
    echo.
    "%PYEXE%" kiro_dashboard.py %*
) else (
    echo Using Python: %PYCMD%
    echo.
    %PYCMD% kiro_dashboard.py %*
)
set EXITCODE=%ERRORLEVEL%

REM Pause on non-clean exit so the window doesn't vanish (130 = Ctrl+C).
if %EXITCODE% NEQ 0 (
    if %EXITCODE% NEQ 130 (
        echo.
        echo [INFO] Process exited with code %EXITCODE%
        pause
    )
)

endlocal
exit /b %EXITCODE%
