# Raises the guest's screen resolution.
#
# Why it matters: the app window is 1200x780 and the guest came up at 1024x768,
# so the right and bottom edges of the window - including part of the settings
# card - are off-screen and under the taskbar. Screenshots of the app are then
# only partly useful, and controls in that region cannot be clicked at all
# because a click at their reported coordinates lands on the taskbar.

param(
  [int]$Width = 1600,
  [int]$Height = 900
)

$ErrorActionPreference = "Stop"

$signature = @'
using System;
using System.Runtime.InteropServices;

public static class Display
{
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Ansi)]
    public struct DEVMODE
    {
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string dmDeviceName;
        public short dmSpecVersion, dmDriverVersion, dmSize, dmDriverExtra;
        public int dmFields;
        public int dmPositionX, dmPositionY;
        public int dmDisplayOrientation, dmDisplayFixedOutput;
        public short dmColor, dmDuplex, dmYResolution, dmTTOption, dmCollate;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string dmFormName;
        public short dmLogPixels;
        public int dmBitsPerPel, dmPelsWidth, dmPelsHeight, dmDisplayFlags, dmDisplayFrequency;
        public int dmICMMethod, dmICMIntent, dmMediaType, dmDitherType, dmReserved1, dmReserved2;
        public int dmPanningWidth, dmPanningHeight;
    }

    [DllImport("user32.dll", CharSet = CharSet.Ansi)]
    public static extern int EnumDisplaySettings(string deviceName, int modeNum, ref DEVMODE devMode);

    [DllImport("user32.dll", CharSet = CharSet.Ansi)]
    public static extern int ChangeDisplaySettings(ref DEVMODE devMode, int flags);

    public const int ENUM_CURRENT_SETTINGS = -1;
    public const int DM_PELSWIDTH = 0x00080000;
    public const int DM_PELSHEIGHT = 0x00100000;
    public const int CDS_UPDATEREGISTRY = 0x00000001;
    public const int DISP_CHANGE_SUCCESSFUL = 0;

    public static string Set(int width, int height)
    {
        // Enumerate first. Asking for a mode the virtual display adapter does not
        // offer fails with DISP_CHANGE_FAILED (-1) and changes nothing, which is
        // what happened with 1600x900 on VMware's SVGA driver.
        DEVMODE mode = new DEVMODE();
        mode.dmSize = (short)Marshal.SizeOf(mode);
        if (EnumDisplaySettings(null, ENUM_CURRENT_SETTINGS, ref mode) == 0)
            return "EnumDisplaySettings failed";
        string before = mode.dmPelsWidth + "x" + mode.dmPelsHeight;

        int bestIndex = -1;
        int bestWidth = 0, bestHeight = 0;
        for (int i = 0; ; i++)
        {
            DEVMODE candidate = new DEVMODE();
            candidate.dmSize = (short)Marshal.SizeOf(candidate);
            if (EnumDisplaySettings(null, i, ref candidate) == 0) break;
            if (candidate.dmBitsPerPel < 32) continue;
            if (candidate.dmPelsWidth < width || candidate.dmPelsHeight < height) continue;
            // Smallest mode that still fits, so the result stays readable.
            if (bestIndex < 0 ||
                candidate.dmPelsWidth * candidate.dmPelsHeight < bestWidth * bestHeight)
            {
                bestIndex = i;
                bestWidth = candidate.dmPelsWidth;
                bestHeight = candidate.dmPelsHeight;
            }
        }
        if (bestIndex < 0) return "was " + before + ", no mode at least " + width + "x" + height;

        DEVMODE chosen = new DEVMODE();
        chosen.dmSize = (short)Marshal.SizeOf(chosen);
        EnumDisplaySettings(null, bestIndex, ref chosen);
        chosen.dmFields = DM_PELSWIDTH | DM_PELSHEIGHT;

        int rc = ChangeDisplaySettings(ref chosen, CDS_UPDATEREGISTRY);
        if (rc != DISP_CHANGE_SUCCESSFUL)
            return "was " + before + ", best candidate " + bestWidth + "x" + bestHeight +
                   ", ChangeDisplaySettings returned " + rc;
        return "was " + before + ", now " + bestWidth + "x" + bestHeight;
    }
}
'@

Add-Type -TypeDefinition $signature -Language CSharp | Out-Null
Write-Output ([Display]::Set($Width, $Height))
