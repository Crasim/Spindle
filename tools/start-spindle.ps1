param([switch]$BuildIfMissing)
$ErrorActionPreference = 'Stop'
$workspace = Split-Path -Parent $PSScriptRoot
$data = if ($env:SPINDLE_DATA_DIR) { $env:SPINDLE_DATA_DIR } else { Join-Path $env:LOCALAPPDATA 'Spindle' }

Add-Type -Namespace SpindleLauncher -Name Window -MemberDefinition @'
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern IntPtr FindWindowEx(IntPtr parent, IntPtr after, string className, string title);

    [DllImport("user32.dll")]
    private static extern uint GetWindowThreadProcessId(IntPtr window, out uint process);

    [DllImport("user32.dll")]
    private static extern bool ShowWindowAsync(IntPtr window, int command);

    [DllImport("user32.dll")]
    private static extern bool SetForegroundWindow(IntPtr window);

    [DllImport("user32.dll")]
    private static extern bool IsIconic(IntPtr window);

    public static bool Restore(uint owner)
    {
        IntPtr window = IntPtr.Zero;
        while ((window = FindWindowEx(IntPtr.Zero, window, null, "Spindle")) != IntPtr.Zero)
        {
            uint process;
            GetWindowThreadProcessId(window, out process);
            if (process != owner)
                continue;

            // Restore a minimized window. Show a tray-hidden window at its existing size.
            ShowWindowAsync(window, IsIconic(window) ? 9 : 5);
            SetForegroundWindow(window);
            return true;
        }
        return false;
    }
'@

# The control endpoint identifies the owner of this workspace, including when its window
# is hidden in the tray. Compare canonical Windows path spellings before using its PID.
foreach ($file in Get-ChildItem -LiteralPath (Join-Path $data 'control') -Filter '*.json' -ErrorAction SilentlyContinue) {
    try {
        $owner = Get-Content -LiteralPath $file.FullName -Raw | ConvertFrom-Json
    } catch {
        continue
    }
    if (!$owner.root -or !$owner.pid) { continue }
    $ownerRoot = $owner.root -replace '^\\\\\?\\UNC\\', '\\' -replace '^\\\\\?\\', ''
    if ($ownerRoot -ne $workspace) { continue }
    $running = Get-Process -Id $owner.pid -ErrorAction SilentlyContinue
    if ($running.ProcessName -eq 'spindle' -and [SpindleLauncher.Window]::Restore($running.Id)) { exit 0 }
}

$build = @('target\release\spindle.exe', 'target\staged\release\spindle.exe', 'target\panels\release\spindle.exe') |
    ForEach-Object { Get-Item -LiteralPath (Join-Path $workspace $_) -ErrorAction SilentlyContinue } |
    Sort-Object LastWriteTimeUtc -Descending |
    Select-Object -First 1
if (!$build -and $BuildIfMissing) {
    $cargo = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
    Push-Location -LiteralPath $workspace
    try {
        & $cargo build --release --locked
        if ($LASTEXITCODE -ne 0) { throw 'Could not build Spindle. Open Start Spindle.cmd for build output.' }
        $build = Get-Item -LiteralPath (Join-Path $workspace 'target\release\spindle.exe')
    } finally {
        Pop-Location
    }
}
if (!$build) { throw 'No Spindle release build was found.' }

$startupError = Join-Path $workspace ("target\spindle-startup-{0}.log" -f $PID)
$env:SPINDLE_GUI_LAUNCH = '1'
$process = Start-Process -FilePath $build.FullName -WorkingDirectory $workspace -WindowStyle Hidden `
    -RedirectStandardError $startupError -PassThru
if ($process.WaitForExit(3000) -and $process.ExitCode -ne 0) {
    Get-Content -LiteralPath $startupError
    Write-Host "Spindle could not open. Startup log: $startupError"
    exit 1
}
