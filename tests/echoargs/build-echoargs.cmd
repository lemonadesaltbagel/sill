@echo off
rem Build c\echoargs.c with MSVC: build-echoargs.cmd <output.exe>
rem The first sketch of `msvc-env`: vswhere locates VS, vcvars64.bat sets INCLUDE/LIB/PATH.
rem vswhere's output goes through a temp file, not `for /f`: cmd's quote-stripping
rem rules break a quoted command whose path contains "(x86)".
setlocal
set "VSWHERE=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe"
set "TMPOUT=%TEMP%\sill-vsinstall-%RANDOM%.txt"
"%VSWHERE%" -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath -utf8 > "%TMPOUT%" || exit /b 1
set /p VSINSTALL=<"%TMPOUT%"
del "%TMPOUT%"
if not defined VSINSTALL (
  echo build-echoargs: vswhere found no Visual Studio with MSVC x64 tools 1>&2
  exit /b 1
)
call "%VSINSTALL%\VC\Auxiliary\Build\vcvars64.bat" >nul || exit /b 1
cl /nologo /utf-8 /W4 /O2 /Fe:"%~1" "%~dp0c\echoargs.c" /link /SUBSYSTEM:CONSOLE || exit /b 1
