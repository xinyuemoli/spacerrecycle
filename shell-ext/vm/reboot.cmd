@echo off
REM Reboot the guest.
REM
REM Run as a file: vmrun appends a stray whitespace argument, and shutdown.exe
REM rejects the whole command line with exit code 1 when it sees one - which
REM looks like a failed reboot rather than a malformed command.
REM
REM A reboot is how the shell comes back after taskkill removed it, and it is
REM also the cleanest way to make Explorer re-read the overlay handler list:
REM AutoAdminLogon is on for the account the guest logs itself in as, so the console returns to
REM the desktop on its own.
shutdown /r /t 5 /f