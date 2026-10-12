@echo off
REM Build the overlay probe DLL with the MSVC toolchain.
setlocal
set VSDIR=C:\Program Files\Microsoft Visual Studio\2022\Community
call "%VSDIR%\VC\Auxiliary\Build\vcvars64.bat" >nul || exit /b 1
cd /d "%~dp0"
cl /nologo /LD /O2 /EHsc /W3 /DUNICODE /D_UNICODE ProbeOverlay.cpp ^
   /link /DEF:ProbeOverlay.def /OUT:SpaceRecycleProbe.dll ole32.lib shell32.lib shlwapi.lib
if errorlevel 1 exit /b 1
echo BUILT: %~dp0SpaceRecycleProbe.dll