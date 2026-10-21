# Crash analysis for the shell extension.
#
# The first attempt at this crash was analysed with a hand-written minidump
# parser, which got as far as "an access violation reading address 0" and no
# further: a truncated dump has no usable stack, and x64 release builds keep no
# frame pointers, so there is nothing to walk. cdb had been installed the whole
# time.
#
# Rules that came out of it:
#   - !analyze -v for the classification, but `kv` for the stack. It is the stack
#     that names the caller, and the caller is the answer.
#   - Read the registers. For a null dereference the argument registers hold the
#     null and, next to it, the string it was compared against.
#   - Copy the crash dump whole. A truncated dump still reports the exception but
#     silently loses the frames above it, which is where the cause lives.
#   - When the interesting memory is not in a truncated dump, read it out of the
#     binary instead. Same build, same RVA.
#
# Usage:  scripts\analyze-dump.ps1 <dump> [symbols]

param(
    [Parameter(Mandatory = $true)][string]$Dump,
    [string]$Symbols = "srv*C:\symbols*https://msdl.microsoft.com/download/symbols"
)

$cdb = "C:\Program Files (x86)\Windows Kits\10\Debuggers\x64\cdb.exe"
if (-not (Test-Path $cdb)) {
    Write-Output "cdb.exe not found. Install the Windows SDK debugging tools."
    exit 2
}

$env:_NT_SYMBOL_PATH = $Symbols

# kv rather than kb: it adds the frame offsets and the arguments, which is what
# turns "crashed in shell32" into "crashed because of a null module name".
& $cdb -z $Dump -c ".ecxr; r; kv; q"