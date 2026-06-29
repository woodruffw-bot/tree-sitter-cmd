@echo off
setlocal enabledelayedexpansion
set SELF=%0
for %%I in (%SELF%) do set APP_HOME=%%~dpI
:home_loop
for %%I in ("%APP_HOME:~1,-1%") do set LEAF=%%~nxI
if not "%LEAF%" == "bin" (
  for %%I in ("%APP_HOME%..") do set APP_HOME=%%~dpfI
  goto home_loop
)
for %%I in ("%APP_HOME%..") do set APP_HOME=%%~dpfI
set APP_CLASSPATH=!APP_HOME!\lib\*
set APP_CLASSPATH=!APP_CLASSPATH!;!APP_HOME!\lib\tools\*
set APP_VERSION=@project.version@
if not defined APP_CONF set APP_CONF=!APP_HOME!\config
cd /d "%APP_HOME%"
if "%1" == "nojava" exit /b 0
if defined JAVA_HOME (set JAVA="%JAVA_HOME%\bin\java.exe") else (set JAVA=java.exe)
%JAVA% -cp "!APP_CLASSPATH!" com.example.Main %*
endlocal
