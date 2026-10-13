@echo off
REM Build the SpaceRecycle delete-interception minifilter.
REM
REM Built with cl/link directly rather than through an MSBuild driver project:
REM the WDK's Visual Studio integration is not installed here, and for a single
REM source file the raw toolchain is both simpler and easier to inspect.
REM
REM Target is Windows 7 (_WIN32_WINNT=0x0601). A down-level driver loads on
REM newer systems too, and the only machine available to test on is the Windows
REM 7 VM. Anything the filter calls must therefore exist in Windows 7's
REM fltmgr.sys - which is why only Vista-era filter APIs appear in the source.
setlocal
set VSDIR=C:\Program Files\Microsoft Visual Studio\2022\Community
call "%VSDIR%\VC\Auxiliary\Build\vcvars64.bat" >nul || exit /b 1

set KIT=C:\Program Files (x86)\Windows Kits\10
set VER=10.0.26100.0
cd /d "%~dp0"

if not exist obj mkdir obj
if not exist bin mkdir bin

echo === compiling srdelete.c ===
REM /D_AMD64_ is what the WDK's MSBuild driver project defines for x64; without
REM it ntdef.h stops at "#error No Target Architecture".
cl /nologo /kernel /c /W4 /O2 ^
   /D_AMD64_ ^
   /D_WIN32_WINNT=0x0601 /DWINVER=0x0601 /DNTDDI_VERSION=0x06010000 ^
   /DPOOL_NX_OPTIN=1 ^
   /I"%KIT%\Include\%VER%\km" ^
   /I"%KIT%\Include\%VER%\km\crt" ^
   /I"%KIT%\Include\%VER%\shared" ^
   /Fo:obj\srdelete.obj src\srdelete.c
if errorlevel 1 exit /b 1

echo === linking srdelete.sys ===
link /nologo /DRIVER /SUBSYSTEM:NATIVE /ENTRY:DriverEntry /MACHINE:X64 ^
   /LIBPATH:"%KIT%\Lib\%VER%\km\x64" ^
   /LIBPATH:"%KIT%\Lib\%VER%\km\crt\x64" ^
   /OUT:bin\srdelete.sys ^
   obj\srdelete.obj fltMgr.lib ntoskrnl.lib ntstrsafe.lib libcntpr.lib BufferOverflowK.lib
if errorlevel 1 exit /b 1

echo.
echo === building srlisten.exe (user-mode listener) ===
cl /nologo /MT /O2 /EHsc /W3 /DUNICODE /D_UNICODE ^
   /I"%KIT%\Include\%VER%\um" ^
   /I"%KIT%\Include\%VER%\shared" ^
   tools\srlisten.cpp ^
   /link /LIBPATH:"%KIT%\Lib\%VER%\um\x64" /OUT:bin\srlisten.exe fltLib.lib
if errorlevel 1 exit /b 1

echo.
echo BUILT:
dir /b bin
