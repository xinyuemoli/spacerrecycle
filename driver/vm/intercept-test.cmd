@echo off
REM Milestone 1 test: does the filter actually see delete attempts?
REM
REM The listener has to run as SYSTEM. FltCreateCommunicationPort was given a
REM NULL security descriptor, so the filter manager applies its default: only
REM LocalSystem and elevated administrators may connect. That is the right
REM default for the product - the thing that answers the delete question will be
REM a service, and a service runs as SYSTEM - but it means an unelevated account
REM cannot connect, which is exactly what happened the first time this was
REM tried (hr=0x80070005).
REM
REM A scheduled task is used to get that identity. The filter has no way to
REM report on its own - DbgPrint needs a kernel debugger, which is not available
REM here - so the evidence has to come back through the port.
REM
REM Both files are expected to disappear: milestone 1 observes, it refuses
REM nothing.
set SRC=C:\sr-test\driver
set OUT=C:\sr-test\intercept-test.txt
set LOG=%SRC%\srlisten.log

if exist "%LOG%" del /q "%LOG%"

echo === start the listener as SYSTEM === > "%OUT%"
schtasks /delete /tn srlisten /f >> "%OUT%" 2>&1
schtasks /create /tn srlisten /tr "\"%SRC%\srlisten.exe\" \"%LOG%\" -seconds 25" /sc once /st 23:59 /ru SYSTEM /rl highest /f >> "%OUT%" 2>&1
echo create exit=%ERRORLEVEL% >> "%OUT%"
schtasks /run /tn srlisten >> "%OUT%" 2>&1
echo run exit=%ERRORLEVEL% >> "%OUT%"
ping -n 6 127.0.0.1 > nul

echo. >> "%OUT%"
echo === fixture === >> "%OUT%"
if not exist C:\sr-test\deltest mkdir C:\sr-test\deltest
echo hello > C:\sr-test\deltest\ordinary.txt
echo hello > C:\sr-test\deltest\marked.txt
dir /b C:\sr-test\deltest >> "%OUT%" 2>&1

echo. >> "%OUT%"
echo === mark one as important (level 0) === >> "%OUT%"
C:\sr-test\bin\menutest.exe set C:\sr-test\deltest\marked.txt 0 2>&1 | findstr /i "state Invoke" >> "%OUT%"
C:\sr-test\bin\menutest.exe get C:\sr-test\deltest\marked.txt >> "%OUT%" 2>&1

echo. >> "%OUT%"
echo === delete the unmarked file === >> "%OUT%"
del C:\sr-test\deltest\ordinary.txt >> "%OUT%" 2>&1
echo del ordinary exit=%ERRORLEVEL% >> "%OUT%"

echo. >> "%OUT%"
echo === delete the marked file === >> "%OUT%"
del C:\sr-test\deltest\marked.txt >> "%OUT%" 2>&1
echo del marked exit=%ERRORLEVEL% >> "%OUT%"

echo. >> "%OUT%"
echo === what remains === >> "%OUT%"
dir /b C:\sr-test\deltest >> "%OUT%" 2>&1

echo. >> "%OUT%"
echo === waiting for the listener to exit === >> "%OUT%"
ping -n 26 127.0.0.1 > nul
schtasks /delete /tn srlisten /f >> "%OUT%" 2>&1

echo. >> "%OUT%"
echo === listener log === >> "%OUT%"
if exist "%LOG%" (type "%LOG%" >> "%OUT%") else (echo (no listener log) >> "%OUT%")

echo done