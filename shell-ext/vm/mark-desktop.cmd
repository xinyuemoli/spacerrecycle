@echo off
REM Applies the marks to the desktop demo files and reports them back.
REM
REM Run as a file: vmrun appends a stray whitespace argument to inline commands.
REM
REM The desktop is the only place a session-0 process can make a GUI change
REM visible, because session 1's own Explorer paints it.
REM
REM %USERPROFILE% rather than a literal path: this harness used to carry the
REM developer's own user name, which made it both a leak and a script that only
REM ran on one machine.
set MENU=C:\sr-test\bin\menutest.exe
set D=%USERPROFILE%\Desktop
set OUT=C:\sr-test\marks.txt

"%MENU%" set "%D%\sr-1-important.txt"  0 > nul 2>&1
"%MENU%" set "%D%\sr-2-important.txt"  0 > nul 2>&1
"%MENU%" set "%D%\sr-3-disposable.txt" 1 > nul 2>&1
"%MENU%" set "%D%\sr-4-disposable.txt" 1 > nul 2>&1
"%MENU%" set "%D%\sr-5-unmarked.txt"   2 > nul 2>&1
"%MENU%" set "%D%\sr-6-unmarked.txt"   2 > nul 2>&1

echo === marks === > "%OUT%"
for %%F in (sr-1-important.txt sr-2-important.txt sr-3-disposable.txt sr-4-disposable.txt sr-5-unmarked.txt sr-6-unmarked.txt) do (
  echo. >> "%OUT%"
  echo --- %%F --- >> "%OUT%"
  "%MENU%" get "%D%\%%F" >> "%OUT%" 2>&1
)

echo done