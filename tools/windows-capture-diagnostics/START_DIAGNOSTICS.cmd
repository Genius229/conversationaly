@echo off
setlocal
set "SCRIPT=%~dp0collect-dshow.ps1"

if not exist "%SCRIPT%" (
  echo ERROR: collect-dshow.ps1 was not found next to this launcher.
  echo Extract the complete ZIP before running diagnostics.
  pause
  exit /b 2
)

set "POWERSHELL=%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe"
if exist "%SystemRoot%\Sysnative\WindowsPowerShell\v1.0\powershell.exe" set "POWERSHELL=%SystemRoot%\Sysnative\WindowsPowerShell\v1.0\powershell.exe"

"%POWERSHELL%" -NoLogo -NoProfile -ExecutionPolicy Bypass -File "%SCRIPT%"
set "RESULT=%ERRORLEVEL%"
echo.
if not "%RESULT%"=="0" echo Diagnostics reported an error. The partial ZIP path is shown above when it could be created.
echo Press any key to close this window.
pause >nul
exit /b %RESULT%
