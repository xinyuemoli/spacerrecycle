@echo off
REM Build the shell extension DLL and its test host.
REM
REM /MT (static CRT) is deliberate. A shell extension is loaded into
REM explorer.exe; depending on a VC runtime that may not be present, or may be
REM a different version than the one Explorer already mapped, is a classic way
REM to make the desktop fail to start.
setlocal
set VSDIR=C:\Program Files\Microsoft Visual Studio\2022\Community
call "%VSDIR%\VC\Auxiliary\Build\vcvars64.bat" >nul || exit /b 1
cd /d "%~dp0"

if not exist bin mkdir bin
if not exist bin\icons mkdir bin\icons
copy /y icons\*.ico bin\icons\ >nul

echo === building SpaceRecycleShell.dll ===
REM /utf-8: the sources contain Chinese menu strings. Without it MSVC reads
REM them in the system code page (936 here), mangles the string literals and
REM fails to parse the file at all.
REM
REM _WIN32_WINNT=0x0601 (Windows 7): the verification VM runs Windows 7 SP1, and
REM restricting the headers to the Win7 API set turns "we accidentally called
REM something newer" into a compile error instead of a DLL that fails to load.
cl /nologo /LD /MT /O2 /EHsc /W3 /utf-8 /DUNICODE /D_UNICODE ^
   /D_WIN32_WINNT=0x0601 /DWINVER=0x0601 ^
   src\SpaceRecycleShell.cpp ^
   /link /DEF:src\SpaceRecycleShell.def /OUT:bin\SpaceRecycleShell.dll ^
   ole32.lib shell32.lib shlwapi.lib advapi32.lib user32.lib
if errorlevel 1 exit /b 1

echo === building menutest.exe ===
cl /nologo /MT /O2 /EHsc /W3 /utf-8 /DUNICODE /D_UNICODE ^
   /D_WIN32_WINNT=0x0601 /DWINVER=0x0601 ^
   tools\menutest.cpp ^
   /link /OUT:bin\menutest.exe ole32.lib shell32.lib shlwapi.lib advapi32.lib
if errorlevel 1 exit /b 1

del /q *.obj 2>nul
echo.
echo BUILT:
dir /b bin