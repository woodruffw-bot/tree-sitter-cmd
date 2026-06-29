@setlocal EnableDelayedExpansion
@echo off
rem Original MRE: an interactive launcher menu exercising the quoted
rem `set /p "name=prompt"` form (prompt with trailing spaces).

set "DEBUGGER="
for /f "tokens=2 delims=[]" %%a in ('ping -n 1 -4 ""') do set "IPADDR=%%a"
if exist "%~dp0tools\dbg.exe" set "DEBUGGER=%~dp0tools\dbg.exe /noauth"

:menu
echo(
echo   1) Run tests
echo   2) Open shell
if defined DEBUGGER echo   3) Attach debugger ^(!IPADDR!^)
echo   Q) Quit
set "answer="
set /p "answer=Enter your choice: "
if /i "!answer!"=="q" goto :eof
if "!answer!"=="1" ( call :runtests & goto menu )
if "!answer!"=="2" ( cmd /k & goto menu )
if "!answer!"=="3" if defined DEBUGGER ( start "" !DEBUGGER! & goto menu )
echo Unrecognized choice: !answer!
goto menu

:runtests
for /f "usebackq tokens=*" %%F in (`dir /b "%~dp0*.tst" 2^>nul`) do (
  echo running %%~nxF
  call "%%F" >> "%TEMP%\run.log" 2>&1
)
exit /b 0
