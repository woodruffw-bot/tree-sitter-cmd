@echo off
setlocal EnableDelayedExpansion
set CP=
for /f "usebackq delims=" %%J in (`dir /b "%~dp0lib\*.jar" 2^>nul`) do (
  set CP=!CP!;%~dp0lib\%%J
)
if "!CP!" == "" (
  echo no jars found 1>&2
  exit /b 1
)
for /f "tokens=2 delims==" %%V in ('set CP') do echo using %%V
java -classpath "!CP:~1!" %* & if errorlevel 1 exit /b !errorlevel!
endlocal & goto :eof
