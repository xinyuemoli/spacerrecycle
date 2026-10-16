# Starts a program on the guest's interactive desktop, from a SYSTEM context.
#
# The guest keeps coming up at the sign-in screen. Windows removes
# AutoAdminLogon and DefaultUserName after a successful auto-logon, so a machine
# that has booted once no longer logs itself in, and re-adding the values does
# not take effect. With nobody logged on there is no session for a scheduled
# task marked /it to run in either, so every route that goes through a logon is
# closed.
#
# This does not need one. Running as SYSTEM - which a scheduled task with
# /ru SYSTEM provides - the account's token can be obtained directly with
# LogonUser, moved into the console session with SetTokenInformation, and used
# with CreateProcessAsUser. The result is a process on session 1's desktop
# owned by that user, which is exactly what was missing.
#
# It must run as SYSTEM, not as the user: SetTokenInformation needs
# SeTcbPrivilege and CreateProcessAsUser needs SeAssignPrimaryTokenPrivilege,
# and an administrator account does not hold either by default.

param(
  [Parameter(Mandatory = $true)][string]$Command,
  [string]$User = "sruser",
  [string]$Domain = "SRWIN11",
  [string]$Password = "<redacted-password>",
  [int]$SessionId = 1
)

$ErrorActionPreference = "Stop"

$signature = @'
using System;
using System.Runtime.InteropServices;

public static class SessionLauncher
{
    [StructLayout(LayoutKind.Sequential)]
    public struct STARTUPINFO
    {
        public int cb;
        public string lpReserved;
        public string lpDesktop;
        public string lpTitle;
        public int dwX, dwY, dwXSize, dwYSize, dwXCountChars, dwYCountChars, dwFillAttribute, dwFlags;
        public short wShowWindow, cbReserved2;
        public IntPtr lpReserved2, hStdInput, hStdOutput, hStdError;
    }

    [StructLayout(LayoutKind.Sequential)]
    public struct PROCESS_INFORMATION
    {
        public IntPtr hProcess, hThread;
        public int dwProcessId, dwThreadId;
    }

    [DllImport("advapi32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    public static extern bool LogonUser(string user, string domain, string password,
        int logonType, int logonProvider, out IntPtr token);

    [DllImport("advapi32.dll", SetLastError = true)]
    public static extern bool DuplicateTokenEx(IntPtr existing, uint access,
        IntPtr attributes, int impersonationLevel, int tokenType, out IntPtr newToken);

    [DllImport("advapi32.dll", SetLastError = true)]
    public static extern bool SetTokenInformation(IntPtr token, int infoClass,
        ref int info, int infoLength);

    [DllImport("advapi32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    public static extern bool CreateProcessAsUser(IntPtr token, string application,
        string commandLine, IntPtr processAttributes, IntPtr threadAttributes,
        bool inheritHandles, uint creationFlags, IntPtr environment,
        string currentDirectory, ref STARTUPINFO startupInfo, out PROCESS_INFORMATION info);

    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern bool CloseHandle(IntPtr handle);

    [DllImport("userenv.dll", SetLastError = true)]
    public static extern bool CreateEnvironmentBlock(out IntPtr environment, IntPtr token, bool inherit);

    [DllImport("userenv.dll", SetLastError = true)]
    public static extern bool DestroyEnvironmentBlock(IntPtr environment);

    public const int LOGON32_LOGON_INTERACTIVE = 2;
    public const int LOGON32_PROVIDER_DEFAULT = 0;
    public const int SecurityImpersonation = 2;
    public const int TokenPrimary = 1;
    public const int TokenSessionId = 12;
    public const uint CREATE_UNICODE_ENVIRONMENT = 0x00000400;

    public static string Launch(string user, string domain, string password,
                                string commandLine, int sessionId)
    {
        IntPtr token;
        if (!LogonUser(user, domain, password, LOGON32_LOGON_INTERACTIVE,
                       LOGON32_PROVIDER_DEFAULT, out token))
            return "LogonUser failed: " + Marshal.GetLastWin32Error();

        IntPtr primary;
        if (!DuplicateTokenEx(token, 0, IntPtr.Zero, SecurityImpersonation,
                              TokenPrimary, out primary))
            return "DuplicateTokenEx failed: " + Marshal.GetLastWin32Error();
        CloseHandle(token);

        int session = sessionId;
        if (!SetTokenInformation(primary, TokenSessionId, ref session, sizeof(int)))
            return "SetTokenInformation failed: " + Marshal.GetLastWin32Error();

        STARTUPINFO si = new STARTUPINFO();
        si.cb = Marshal.SizeOf(si);
        si.lpDesktop = "winsta0\\default";

        // The child has to get the *user's* environment. Passing IntPtr.Zero
        // hands it the launcher's, which is SYSTEM's, and a program started that
        // way inherits SYSTEM's USERPROFILE and TEMP - WebView2 then has nowhere
        // sensible to put its user data folder and the process exits without
        // ever drawing a window. The first attempt did exactly that: the
        // launcher reported a pid, and the process was gone moments later.
        IntPtr environment;
        bool haveEnvironment = CreateEnvironmentBlock(out environment, primary, false);

        PROCESS_INFORMATION pi;
        bool ok = CreateProcessAsUser(primary, null, commandLine, IntPtr.Zero, IntPtr.Zero,
                                      false, CREATE_UNICODE_ENVIRONMENT,
                                      haveEnvironment ? environment : IntPtr.Zero,
                                      null, ref si, out pi);
        int error = ok ? 0 : Marshal.GetLastWin32Error();
        if (haveEnvironment) { DestroyEnvironmentBlock(environment); }

        if (!ok)
            return "CreateProcessAsUser failed: " + error
                 + (haveEnvironment ? "" : " (and no environment block was built)");

        int pid = pi.dwProcessId;
        CloseHandle(pi.hProcess);
        CloseHandle(pi.hThread);
        CloseHandle(primary);
        return "started pid " + pid + " in session " + sessionId;
    }
}
'@

Add-Type -TypeDefinition $signature -Language CSharp | Out-Null

$who = [System.Security.Principal.WindowsIdentity]::GetCurrent()
Write-Output "launcher identity: $($who.Name)"
Write-Output "launching in session ${SessionId}: $Command"
$result = [SessionLauncher]::Launch($User, $Domain, $Password, $Command, $SessionId)
Write-Output $result
if ($result -notmatch "^started pid") { exit 1 }
