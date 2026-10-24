# Meeting Notes one-click updater for Windows. The widget's Update button runs a COPY of this file
# (checking out a new tag rewrites the original), detached from the app. It never prompts.
#
#   powershell.exe -NoProfile -ExecutionPolicy Bypass -File update.ps1 -Repo <abs repo dir> -Status <abs status file>
#
# It writes the status file (state running, then up_to_date, done or failed) and update.log next to it.
# A failure always ends in state "failed", never a status stuck on "running".
# This file is ASCII only on purpose: Windows PowerShell 5.1 reads a file without a BOM as ANSI.
param(
    [string]$Repo = '',
    [string]$Status = ''
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$env:GIT_TERMINAL_PROMPT = '0'
$env:GCM_INTERACTIVE = 'never'

$script:From = 'unknown'
$script:To = $null
$script:FinalWritten = $false
$script:AppMayBeDown = $false
$script:ExitCode = 1
$script:Log = ''
$script:Git = ''
$script:SetupLines = @()
$script:SetupCode = 0
$script:Restored = $true
$Utf8NoBom = New-Object System.Text.UTF8Encoding($false)

# ---------------------------------------------------------------- helpers

# JSON with every non-ASCII character escaped, so the console code page cannot damage it.
function ConvertTo-AsciiJson($Object) {
    $json = ConvertTo-Json -InputObject $Object -Compress -Depth 5
    return [regex]::Replace($json, '[^\x00-\x7F]', { param($m) '\u{0:x4}' -f [int][char]$m.Value })
}

# ISO 8601 UTC time. Invariant culture, so ':' stays a colon on every language setting.
function Get-Now {
    return (Get-Date).ToUniversalTime().ToString("yyyy-MM-dd'T'HH:mm:ss'Z'", [System.Globalization.CultureInfo]::InvariantCulture)
}

function Write-Log([string]$Text) {
    if (-not $script:Log) { return }
    try {
        $line = (Get-Now) + ' ' + $Text + "`r`n"
        [System.IO.File]::AppendAllText($script:Log, $line, $Utf8NoBom)
    } catch { }
}

# Atomic: write <file>.tmp, then rename it over the status file.
function Write-Status([string]$State, [string]$ToTag, [string]$ErrorText) {
    $toValue = $null
    if ($ToTag) { $toValue = $ToTag }
    $errValue = $null
    if ($ErrorText) { $errValue = $ErrorText }
    $obj = [ordered]@{
        state      = $State
        from       = $script:From
        to         = $toValue
        error      = $errValue
        log        = $script:Log
        updated_at = (Get-Now)
        seen       = $false
    }
    $tmp = $Status + '.tmp'
    [System.IO.File]::WriteAllText($tmp, (ConvertTo-AsciiJson $obj), $Utf8NoBom)
    if (Test-Path -LiteralPath $Status) {
        [System.IO.File]::Replace($tmp, $Status, $null)
    } else {
        [System.IO.File]::Move($tmp, $Status)
    }
}

# Runs a program, never throws on its stderr text (Windows PowerShell 5.1 would), returns the exit code and the lines.
# With -ToLog every line is also appended to update.log as it arrives.
function Invoke-Native([string]$File, [string[]]$Arguments, [bool]$ToLog = $false) {
    $old = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    $lines = New-Object System.Collections.ArrayList
    $code = 1
    try {
        & $File @Arguments 2>&1 | ForEach-Object {
            $text = [string]$_
            [void]$lines.Add($text)
            if ($ToLog) { try { [System.IO.File]::AppendAllText($script:Log, $text + "`r`n", $Utf8NoBom) } catch { } }
        }
        $code = $LASTEXITCODE
    } catch {
        [void]$lines.Add($_.Exception.Message)
    } finally {
        $ErrorActionPreference = $old
    }
    return [pscustomobject]@{ Code = $code; Lines = @($lines) }
}

function Find-Tool([string]$ToolName, [string[]]$Fallbacks) {
    $cmd = Get-Command $ToolName -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($cmd) { return [string]$cmd.Source }
    foreach ($f in $Fallbacks) {
        if ($f -and (Test-Path -LiteralPath $f)) { return $f }
    }
    return ''
}

function Get-LastLine([string[]]$Lines) {
    $text = [string]($Lines | Where-Object { $_ -and $_.Trim() } | Select-Object -Last 1)
    $text = $text.Trim()
    if ($text.Length -gt 300) { $text = $text.Substring(0, 300) }
    return $text
}

function Invoke-Git([string[]]$GitArguments) {
    $all = @('-C', $Repo, '-c', 'http.lowSpeedLimit=1000', '-c', 'http.lowSpeedTime=60') + $GitArguments
    return Invoke-Native $script:Git $all
}

# The installed Meeting Notes.exe: the uninstall entry first, then the usual per-user folders. Empty string if none.
function Find-AppExe {
    $dataDir = Join-Path $env:LOCALAPPDATA 'MeetingNotes'
    $dirs = New-Object System.Collections.ArrayList
    $uninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall'
    foreach ($k in @(Get-ChildItem -Path $uninstallKey -ErrorAction SilentlyContinue)) {
        $p = Get-ItemProperty -LiteralPath $k.PSPath -ErrorAction SilentlyContinue
        if ($p -and $p.DisplayName -and ([string]$p.DisplayName -like '*Meeting Notes*')) {
            if ($p.InstallLocation) { [void]$dirs.Add(([string]$p.InstallLocation).Trim('"')) }
            if ($p.DisplayIcon) {
                $icon = ([string]$p.DisplayIcon).Trim('"') -replace ',\s*-?\d+$', ''
                [void]$dirs.Add((Split-Path -Parent $icon))
            }
        }
    }
    foreach ($root in @($env:LOCALAPPDATA, (Join-Path $env:LOCALAPPDATA 'Programs'))) {
        foreach ($d in @(Get-ChildItem -LiteralPath $root -Directory -ErrorAction SilentlyContinue)) {
            if (($d.Name -match 'meeting.?notes') -and ($d.FullName -ne $dataDir)) { [void]$dirs.Add($d.FullName) }
        }
    }
    foreach ($d in $dirs) {
        if ((-not $d) -or (-not (Test-Path -LiteralPath $d -PathType Container))) { continue }
        $exes = @(Get-ChildItem -LiteralPath $d -Filter '*.exe' -File -ErrorAction SilentlyContinue |
            Where-Object { $_.Name -notmatch '^(uninstall|unins|update)' })
        $best = $exes | Where-Object { $_.Name -match 'meeting' } | Select-Object -First 1
        if (-not $best) { $best = $exes | Select-Object -First 1 }
        if ($best) { return [string]$best.FullName }
    }
    return ''
}

# Start the widget again if it is not running. Best effort: never fails the script.
function Start-AppIfStopped {
    try {
        $exe = Find-AppExe
        if (-not $exe) { return }
        $procName = [IO.Path]::GetFileNameWithoutExtension($exe)
        if (@(Get-Process -Name $procName -ErrorAction SilentlyContinue).Count -gt 0) { return }
        Write-Log "Starting the widget again: $exe"
        Start-Process -FilePath $exe | Out-Null
    } catch {
        Write-Log "Could not start the widget again: $($_.Exception.Message)"
    }
}

# The one place a final status is written.
function Complete-Update([string]$State, [string]$ToTag, [string]$ErrorText) {
    $script:FinalWritten = $true
    Write-Status $State $ToTag $ErrorText
    $suffix = ''
    if ($ErrorText) { $suffix = " ($ErrorText)" }
    Write-Log "Finished: $State$suffix"
}

function Stop-Failed([string]$ErrorText) {
    Complete-Update 'failed' $script:To $ErrorText
    if ($script:AppMayBeDown) { Start-AppIfStopped }
    $script:ExitCode = 1
}

# Runs the checked-out setup script with -Update. Output goes to the log and to $script:SetupLines.
# Setup quits the app, so from here on a failure must start it again.
function Invoke-Setup {
    $script:AppMayBeDown = $true
    $setup = Join-Path $Repo 'scripts\setup\windows.ps1'
    $ps = Find-Tool 'powershell.exe' @((Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'))
    if (-not $ps) { $ps = 'powershell.exe' }
    Write-Log 'Running windows.ps1 -Update'
    $run = Invoke-Native $ps @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', $setup, '-Update') $true
    $script:SetupLines = $run.Lines
    $script:SetupCode = $run.Code
}

# The parsed last SETUP_RESULT line of the setup run, or $null.
function Get-SetupResult {
    $line = [string]($script:SetupLines | Where-Object { $_ -like 'SETUP_RESULT *' } | Select-Object -Last 1)
    if (-not $line) { return $null }
    try { return ($line.Substring('SETUP_RESULT '.Length) | ConvertFrom-Json) } catch { return $null }
}

function Test-SetupOk {
    $r = Get-SetupResult
    return [bool]($r -and ($r.ok -eq $true))
}

# The most useful line of a failed setup run.
function Get-SetupError {
    $msg = ''
    $r = Get-SetupResult
    if ($r -and $r.error) { $msg = [string]$r.error }
    if (-not $msg) {
        $errLine = [string]($script:SetupLines | Where-Object { $_ -like 'ERROR: *' } | Select-Object -Last 1)
        if ($errLine) { $msg = $errLine.Substring('ERROR: '.Length) }
    }
    if (-not $msg) { $msg = Get-LastLine @($script:SetupLines | Where-Object { $_ -notlike 'SETUP_RESULT *' }) }
    if (-not $msg) { $msg = "The setup script failed (exit code $($script:SetupCode))." }
    if ($msg.Length -gt 300) { $msg = $msg.Substring(0, 300) }
    return $msg
}

# Put the folder (and the app) back on the old tag. $RunSetup: run setup again to reinstall the old app.
function Restore-Old([bool]$RunSetup) {
    $script:Restored = $true
    if ($script:From -eq 'unknown') { return }
    Write-Log "Going back to $($script:From)"
    $co = Invoke-Git @('checkout', '-f', $script:From)
    foreach ($l in $co.Lines) { Write-Log $l }
    if ($co.Code -ne 0) {
        Write-Log "Could not check out $($script:From) again."
        $script:Restored = $false
        return
    }
    if ($RunSetup) {
        Invoke-Setup
        if (-not (Test-SetupOk)) {
            Write-Log "Restoring $($script:From) failed: $(Get-SetupError)"
            $script:Restored = $false
        }
    }
}

# ---------------------------------------------------------------- the update

function Invoke-Update {
    # 1. running
    $script:Git = Find-Tool 'git' @(
        ($env:ProgramFiles + '\Git\cmd\git.exe'),
        ($env:LOCALAPPDATA + '\Programs\Git\cmd\git.exe'),
        (${env:ProgramFiles(x86)} + '\Git\cmd\git.exe'))
    if ($script:Git) {
        $d = Invoke-Git @('describe', '--tags', '--exact-match', '--match', 'v[0-9]*')
        $tag = ([string]($d.Lines | Select-Object -First 1)).Trim()
        if (($d.Code -eq 0) -and ($tag -match '^v\d')) { $script:From = $tag }
    }
    Write-Status 'running' $null $null
    Write-Log "Update started. Installed version: $($script:From). Repo: $Repo"

    if (-not $script:Git) { Stop-Failed 'Git is missing. Ask Claude to repair the install.'; return }
    $gd = Invoke-Git @('rev-parse', '--git-dir')
    if ($gd.Code -ne 0) { Stop-Failed 'The Meeting Notes install folder is damaged. Ask Claude to repair the install.'; return }

    # 2. newest tag
    $fetch = Invoke-Git @('fetch', '--tags', '--force', 'origin')
    if ($fetch.Code -ne 0) {
        foreach ($l in $fetch.Lines) { Write-Log $l }
        Stop-Failed ('Could not check for updates: ' + (Get-LastLine $fetch.Lines) + '. Check the internet connection.')
        return
    }
    $nt = Invoke-Git @('for-each-ref', '--sort=-v:refname', '--count=1', '--format=%(refname:short)', 'refs/tags/v*')
    $newest = ''
    if ($nt.Code -eq 0) { $newest = ([string]($nt.Lines | Select-Object -First 1)).Trim() }
    if (-not $newest) { Stop-Failed 'No release was found.'; return }
    Write-Log "Newest release: $newest"

    # 3. up to date
    $script:To = $newest
    if ($newest -eq $script:From) {
        Complete-Update 'up_to_date' $newest $null
        $script:ExitCode = 0
        return
    }

    # 4. new version
    $co = Invoke-Git @('checkout', '-f', $newest)
    if ($co.Code -ne 0) {
        foreach ($l in $co.Lines) { Write-Log $l }
        Restore-Old $false
        Stop-Failed ("Could not switch to $newest" + ': ' + (Get-LastLine $co.Lines))
        return
    }
    if (-not (Test-Path -LiteralPath (Join-Path $Repo 'scripts\setup\windows.ps1'))) {
        Restore-Old $false
        Stop-Failed "$newest has no windows.ps1. Ask Claude to repair the install."
        return
    }

    Invoke-Setup
    if (-not (Test-SetupOk)) {
        $msg = Get-SetupError
        Write-Log "Setup failed: $msg"
        if ($msg -like '*still being prepared*') {
            $msg = 'This version is still being prepared. Please click Update again in about 30 minutes.'
            # Nothing was replaced when setup stopped before its step 3 (the app is replaced inside step 2).
            $reached3 = @($script:SetupLines | Where-Object { $_ -like '[[]3/*' }).Count -gt 0
            Restore-Old $reached3
        } else {
            Restore-Old $true
        }
        if (-not $script:Restored) { $msg = "$msg (Going back to $($script:From) also failed.)" }
        Stop-Failed $msg
        return
    }

    # 5. health check
    $mn = Join-Path $env:LOCALAPPDATA 'MeetingNotes\bin\mn.exe'
    $msg = ''
    if (-not (Test-Path -LiteralPath $mn)) {
        $msg = "The mn tool was not found at $mn after the update."
    } else {
        Write-Log 'Running mn doctor'
        $env:MN_EXPECTED_VERSION = $newest.TrimStart('v')
        $doc = Invoke-Native $mn @('doctor') $true
        if ($doc.Code -eq 0) {
            # 6. success
            $result = Get-SetupResult
            if ($result -and ($result.launched -eq $false)) { Start-AppIfStopped }
            Complete-Update 'done' $newest $null
            $script:ExitCode = 0
            return
        }
        $failLine = [string]($doc.Lines | Where-Object { $_ -match '^FAIL' } | Select-Object -First 1)
        if ($failLine) {
            $msg = ($failLine -replace '^FAIL\s+', '').Trim()
            if ($msg.Length -gt 300) { $msg = $msg.Substring(0, 300) }
        } else {
            $msg = "The health check (mn doctor) failed (exit code $($doc.Code))."
        }
    }

    # 7. failure after setup
    Write-Log "Health check failed: $msg"
    Restore-Old $true
    if (-not $script:Restored) { $msg = "$msg (Going back to $($script:From) also failed.)" }
    Stop-Failed $msg
}

# ---------------------------------------------------------------- main

try {
    if ((-not $Repo) -or (-not $Status)) {
        [Console]::Error.WriteLine('Usage: update.ps1 -Repo <abs repo dir> -Status <abs status file>')
        $script:FinalWritten = $true
        $script:ExitCode = 2
    } else {
        $statusDir = Split-Path -Parent $Status
        New-Item -ItemType Directory -Force -Path $statusDir | Out-Null
        $script:Log = Join-Path $statusDir 'update.log'
        try { [System.IO.File]::WriteAllText($script:Log, '', $Utf8NoBom) } catch { }
        Invoke-Update
    }
} catch {
    Write-Log ('The script stopped unexpectedly: ' + $_.Exception.Message + ' at ' + $_.InvocationInfo.PositionMessage)
    try {
        Complete-Update 'failed' $script:To 'The update stopped unexpectedly. Ask Claude to repair the install.'
    } catch { }
    Start-AppIfStopped
    $script:ExitCode = 1
} finally {
    if ((-not $script:FinalWritten) -and $Status) {
        # Stopped from outside (for example the window was closed): never leave the status on "running".
        try { Write-Status 'failed' $script:To 'The update stopped unexpectedly. Ask Claude to repair the install.' } catch { }
        Start-AppIfStopped
    }
}
exit $script:ExitCode
