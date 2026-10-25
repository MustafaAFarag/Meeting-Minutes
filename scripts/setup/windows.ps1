# Meeting Notes setup for Windows. Safe to run again: it updates in place and never touches
# your notes or your My Meetings page address. Works in Windows PowerShell 5.1 and PowerShell 7.
#
#   powershell -ExecutionPolicy Bypass -File scripts\setup\windows.ps1 -DetectName
#   powershell -ExecutionPolicy Bypass -File scripts\setup\windows.ps1 -Name "Full Name" `
#       [-Model small|turbo] [-SkipPage] [-SkipLaunch] [-ReleaseBase <url-or-dir>]
#   powershell -ExecutionPolicy Bypass -File scripts\setup\windows.ps1 -Update
#       (an update: the name comes from the existing config)
#
# Last line of output: SETUP_RESULT {json}
# This file is ASCII only on purpose: Windows PowerShell 5.1 reads a file without a BOM as ANSI.
param(
    [string]$Name = '',
    [ValidateSet('', 'small', 'turbo')]
    [string]$Model = '',
    [switch]$SkipPage,
    [switch]$SkipLaunch,
    [switch]$Update,
    # Default (empty): the GitHub release of the tag this folder is checked out at, or releases/latest/download
    # if it is not at a tag (a dev checkout of main, or no git). A value given here always wins.
    [string]$ReleaseBase = '',
    # Hidden, for tests.
    [string]$WhisperCli = '',
    [string]$WhisperModel = '',
    [switch]$DetectName
)

# JSON with every non-ASCII character escaped, so the console code page cannot damage it.
function ConvertTo-AsciiJson($Object) {
    $json = ConvertTo-Json -InputObject $Object -Compress -Depth 5
    return [regex]::Replace($json, '[^\x00-\x7F]', { param($m) '\u{0:x4}' -f [int][char]$m.Value })
}

function Get-UsableDetectedName([object]$Value, [string]$LoginName) {
    if ($null -eq $Value) { return $null }
    $candidate = ([string]$Value).Trim()
    if ([string]::IsNullOrWhiteSpace($candidate)) { return $null }
    if ($LoginName -and [string]::Equals($candidate, $LoginName, [System.StringComparison]::OrdinalIgnoreCase)) { return $null }
    if (@('admin', 'administrator', 'user', 'owner', 'root', 'guest', 'microsoft account') -contains $candidate) { return $null }
    if ($candidate -notmatch '\p{L}') { return $null }
    return $candidate
}

function ConvertFrom-EmailNameGuess([string]$Email) {
    if ([string]::IsNullOrWhiteSpace($Email)) { return $null }
    $atIndex = $Email.IndexOf('@')
    if ($atIndex -le 0) { return $null }
    $localPart = $Email.Substring(0, $atIndex)
    if ($localPart -notmatch '^[A-Za-z]+(?:[._-][A-Za-z]+)+$') { return $null }
    $parts = @($localPart -split '[._-]')
    if ($parts.Count -lt 2) { return $null }
    $textInfo = [System.Globalization.CultureInfo]::InvariantCulture.TextInfo
    $titleParts = @()
    foreach ($part in $parts) {
        $titleParts += $textInfo.ToTitleCase($part.ToLowerInvariant())
    }
    return ($titleParts -join ' ')
}

function Find-DetectionClaude {
    $cmd = Get-Command 'claude' -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($cmd) { return [string]$cmd.Source }
    $userHome = [Environment]::GetFolderPath('UserProfile')
    foreach ($path in @(
        (Join-Path $userHome '.local\bin\claude.exe'),
        (Join-Path $env:APPDATA 'npm\claude.cmd'),
        (Join-Path $userHome '.claude\local\claude.exe')
    )) {
        if ($path -and (Test-Path -LiteralPath $path)) { return $path }
    }
    return ''
}

# Run auth status without files and terminate it if it takes more than 3.5 seconds.
function Get-DetectionClaudeStatus([string]$Claude) {
    $process = $null
    try {
        $startInfo = New-Object System.Diagnostics.ProcessStartInfo
        if ([IO.Path]::GetExtension($Claude) -match '^\.(cmd|bat)$') {
            $startInfo.FileName = $env:ComSpec
            $startInfo.Arguments = '/d /s /c ""' + $Claude + '" auth status"'
        } else {
            $startInfo.FileName = $Claude
            $startInfo.Arguments = 'auth status'
        }
        $startInfo.UseShellExecute = $false
        $startInfo.RedirectStandardOutput = $true
        $startInfo.RedirectStandardError = $true
        $startInfo.CreateNoWindow = $true
        $process = New-Object System.Diagnostics.Process
        $process.StartInfo = $startInfo
        if (-not $process.Start()) { return $null }
        $stdout = $process.StandardOutput.ReadToEndAsync()
        $stderr = $process.StandardError.ReadToEndAsync()
        if (-not $process.WaitForExit(3500)) {
            try { $process.Kill() } catch { }
            return $null
        }
        $process.WaitForExit()
        if ($process.ExitCode -ne 0) { return $null }
        return [string]$stdout.Result
    } catch {
        return $null
    } finally {
        if ($process) { $process.Dispose() }
    }
}

if ($DetectName) {
    $detectedName = $null
    $detectedSource = $null
    $candidate = $null

    try {
        $office = Get-ItemProperty -LiteralPath 'HKCU:\Software\Microsoft\Office\Common\UserInfo' -Name 'UserName' -ErrorAction Stop
        $candidate = [string]$office.UserName
        $detectedName = Get-UsableDetectedName $candidate $env:USERNAME
        if ($detectedName) { $detectedSource = 'office' }
    } catch { }

    if (-not $detectedName) {
        try {
            [void](Add-Type -AssemblyName System.DirectoryServices.AccountManagement -ErrorAction Stop)
            $candidate = [System.DirectoryServices.AccountManagement.UserPrincipal]::Current.DisplayName
            $detectedName = Get-UsableDetectedName $candidate $env:USERNAME
            if ($detectedName) { $detectedSource = 'windows-account' }
        } catch { }
    }

    if (-not $detectedName) {
        try {
            $localUser = Get-LocalUser -Name $env:USERNAME -ErrorAction Stop
            $candidate = [string]$localUser.FullName
            $detectedName = Get-UsableDetectedName $candidate $env:USERNAME
            if ($detectedName) { $detectedSource = 'windows-account' }
        } catch { }
    }

    if (-not $detectedName) {
        try {
            $claude = Find-DetectionClaude
            if ($claude) {
                $status = Get-DetectionClaudeStatus $claude
                if ($status) {
                    $statusObject = ConvertFrom-Json -InputObject $status -ErrorAction Stop
                    $guess = ConvertFrom-EmailNameGuess ([string]$statusObject.email)
                    $detectedName = Get-UsableDetectedName $guess $env:USERNAME
                    if ($detectedName) { $detectedSource = 'claude-email-guess' }
                }
            }
        } catch { }
    }

    $result = [ordered]@{ name = $detectedName; source = $detectedSource }
    [Console]::Out.WriteLine('DETECTED_NAME ' + (ConvertTo-AsciiJson $result))
    exit 0
}

$ErrorActionPreference = 'Stop'
# Invoke-WebRequest is very slow with the progress bar on 5.1.
$ProgressPreference = 'SilentlyContinue'
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

$WhisperUrl = 'https://github.com/ggml-org/whisper.cpp/releases/download/b5130/whisper-bin-x64.zip'
$ModelUrl = 'https://huggingface.co/ggerganov/whisper.cpp/resolve/main'
$VadUrl = 'https://huggingface.co/ggml-org/whisper-vad/resolve/main'
$ClaudeInstallUrl = 'https://claude.ai/install.ps1'
$LatestBase = 'https://github.com/MustafaAFarag/Meeting-Minutes/releases/latest/download'
$TagBase = 'https://github.com/MustafaAFarag/Meeting-Minutes/releases/download'
$ReleaseTag = ''
$ReleaseLabel = ''
$Total = 8
$script:StepNumber = 0
$script:Warnings = New-Object System.Collections.ArrayList

# ---------------------------------------------------------------- helpers

function Write-Step([string]$Text) {
    $script:StepNumber = $script:StepNumber + 1
    Write-Host ('[{0}/{1}] {2}' -f $script:StepNumber, $Total, $Text)
}

function Add-Warning([string]$Text) {
    Write-Host "WARNING: $Text"
    [void]$script:Warnings.Add($Text)
}

function Fail([string]$Text) {
    [Console]::Error.WriteLine("ERROR: $Text")
    $result = [ordered]@{ ok = $false; update = [bool]$Update; error = $Text }
    Write-Output ('SETUP_RESULT ' + (ConvertTo-AsciiJson $result))
    exit 1
}

function ConvertTo-TomlString([string]$Text) {
    return '"' + $Text.Replace('\', '\\').Replace('"', '\"') + '"'
}

# A single-quoted TOML literal string keeps Windows backslashes as they are.
function ConvertTo-TomlPath([string]$Text) {
    if ($Text.Contains("'")) { return ConvertTo-TomlString $Text }
    return "'" + $Text + "'"
}

# Runs a program, never throws on its stderr text (Windows PowerShell 5.1 would), returns the exit code and the lines.
function Invoke-Native([string]$File, [string[]]$Arguments) {
    $old = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    $lines = @()
    $code = 1
    try {
        $lines = @(& $File @Arguments 2>&1 | ForEach-Object { [string]$_ })
        $code = $LASTEXITCODE
    } catch {
        $lines = @($_.Exception.Message)
    } finally {
        $ErrorActionPreference = $old
    }
    return [pscustomobject]@{ Code = $code; Lines = $lines }
}

function Find-Tool([string]$ToolName, [string[]]$Fallbacks) {
    $cmd = Get-Command $ToolName -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($cmd) { return [string]$cmd.Source }
    foreach ($f in $Fallbacks) {
        if ($f -and (Test-Path -LiteralPath $f)) { return $f }
    }
    return ''
}

# Downloads to <Dest>.part first, so a broken download never looks finished.
function Get-Download([string]$Url, [string]$Dest) {
    $part = $Dest + '.part'
    $ok = $false
    $notFound = $false
    $lastError = ''
    for ($i = 1; ($i -le 3) -and (-not $ok) -and (-not $notFound); $i++) {
        try {
            if (Test-Path -LiteralPath $part) { Remove-Item -LiteralPath $part -Force }
            Invoke-WebRequest -UseBasicParsing -Uri $Url -OutFile $part
            $ok = $true
        } catch {
            $lastError = $_.Exception.Message
            $status = 0
            try { $status = [int]$_.Exception.Response.StatusCode } catch { }
            if ($status -eq 404) { $notFound = $true } else { Start-Sleep -Seconds 3 }
        }
    }
    if ((-not $ok) -and $notFound -and $ReleaseTag -and $Url.StartsWith($ReleaseBase.TrimEnd('/') + '/')) {
        # A tag whose release CI is still building has no files yet.
        Fail 'This version is still being prepared. Wait 30 minutes, then run this script again.'
    }
    if (-not $ok) {
        Fail "Could not download $Url. Check the internet connection, then run this script again. ($lastError)"
    }
    Move-Item -LiteralPath $part -Destination $Dest -Force
}

# A file from the release base: a URL or a local folder.
function Get-Release([string]$FileName, [string]$Dest) {
    if (($ReleaseBase -notmatch '^https?://') -and (Test-Path -LiteralPath $ReleaseBase -PathType Container)) {
        $src = Join-Path $ReleaseBase $FileName
        if (-not (Test-Path -LiteralPath $src)) { Fail "$ReleaseBase has no file named $FileName." }
        Copy-Item -LiteralPath $src -Destination $Dest -Force
    } else {
        Get-Download ($ReleaseBase.TrimEnd('/') + '/' + $FileName) $Dest
    }
}

# The installed Meeting Notes.exe: the uninstall entry first, then the usual per-user folders. Empty string if none.
function Find-AppExe {
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
            if (($d.Name -match 'meeting.?notes') -and ($d.FullName -ne $DataDir)) { [void]$dirs.Add($d.FullName) }
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

function Stop-App([string]$ExePath) {
    if (-not $ExePath) { return }
    $procName = [IO.Path]::GetFileNameWithoutExtension($ExePath)
    $running = @(Get-Process -Name $procName -ErrorAction SilentlyContinue)
    if ($running.Count -gt 0) {
        $running | Stop-Process -Force -ErrorAction SilentlyContinue
        Start-Sleep -Seconds 2
    }
}

# Makes a shortcut to the app unless one that points at it is already there. Throws on failure.
function Set-AppShortcut([string]$LinkPath, [string]$ExePath) {
    $shell = New-Object -ComObject WScript.Shell
    if (Test-Path -LiteralPath $LinkPath) {
        $existing = $shell.CreateShortcut($LinkPath)
        if ([string]$existing.TargetPath -ieq $ExePath) { return }
    }
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $LinkPath) | Out-Null
    $link = $shell.CreateShortcut($LinkPath)
    $link.TargetPath = $ExePath
    $link.WorkingDirectory = Split-Path -Parent $ExePath
    $link.IconLocation = $ExePath + ',0'
    $link.Description = 'Meeting Notes'
    $link.Save()
}

# The Desktop icon, and a Start-menu entry if the installer did not make one.
function Add-AppShortcuts([string]$ExePath) {
    $desktop = [Environment]::GetFolderPath('Desktop')
    Set-AppShortcut (Join-Path $desktop 'Meeting Notes.lnk') $ExePath
    $programs = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs'
    $found = @(Get-ChildItem -LiteralPath $programs -Recurse -Filter 'Meeting Notes*.lnk' -File -ErrorAction SilentlyContinue)
    if ($found.Count -eq 0) { Set-AppShortcut (Join-Path $programs 'Meeting Notes.lnk') $ExePath }
}

# ---------------------------------------------------------------- main
try {
    $UserHome = [Environment]::GetFolderPath('UserProfile')
    $ConfigDir = Join-Path $env:APPDATA 'meeting-notes'
    $Config = Join-Path $ConfigDir 'config.toml'
    $UrlFile = Join-Path $ConfigDir 'artifact_url.txt'
    $DataDir = Join-Path $env:LOCALAPPDATA 'MeetingNotes'
    $BinDir = Join-Path $DataDir 'bin'
    $Mn = Join-Path $BinDir 'mn.exe'

    # ------------------------------------------------------------ 1. checks
    Write-Step 'Checking this computer'
    if (-not [Environment]::Is64BitOperatingSystem) { Fail 'Only 64-bit Windows 10 or 11 is supported.' }

    # Where the binaries come from. An explicit -ReleaseBase always wins. Otherwise: the release of the tag
    # this repo folder is checked out at (so going back to an old tag installs the old binaries), or the
    # latest release (a dev checkout of main, or no git).
    if ($ReleaseBase) {
        $ReleaseBase = $ReleaseBase.TrimEnd('/')
        $ReleaseLabel = 'custom'
        # The files are run right after the download: only this project's releases or a local folder.
        $ownReleases = $TagBase.Substring(0, $TagBase.Length - '/download'.Length) + '/'
        $isOwn = $ReleaseBase.StartsWith($ownReleases, [System.StringComparison]::OrdinalIgnoreCase)
        $isFolder = ($ReleaseBase -notmatch '^[a-zA-Z][a-zA-Z0-9+.-]*://') -and (Test-Path -LiteralPath $ReleaseBase -PathType Container)
        if ((-not $isOwn) -and (-not $isFolder) -and ($env:MN_ALLOW_CUSTOM_RELEASE_BASE -ne '1')) {
            Fail '-ReleaseBase must be a local folder or a release of this project on GitHub.'
        }
        Write-Host "Using release files from $ReleaseBase"
    } else {
        $ReleaseBase = $LatestBase
        $ReleaseLabel = 'latest'
        try {
            $repoDir = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
            $gitExe = Find-Tool 'git' @(
                ($env:ProgramFiles + '\Git\cmd\git.exe'),
                (${env:ProgramFiles(x86)} + '\Git\cmd\git.exe'),
                ($env:LOCALAPPDATA + '\Programs\Git\cmd\git.exe'))
            if ($repoDir -and $gitExe) {
                $desc = Invoke-Native $gitExe @('-C', $repoDir, 'describe', '--tags', '--exact-match', '--match', 'v[0-9]*')
                $descTag = ([string]($desc.Lines | Select-Object -First 1)).Trim()
                if (($desc.Code -eq 0) -and ($descTag -match '^v\d')) {
                    $ReleaseTag = $descTag
                    $ReleaseLabel = $descTag
                    $ReleaseBase = $TagBase + '/' + $descTag
                }
            }
        } catch { }
        Write-Host "Using release $ReleaseLabel"
    }

    if ($Update -and (-not (Test-Path -LiteralPath $Config))) {
        Fail "There is no earlier install to update (no config file at $Config). Do a normal install instead: follow SETUP.md from step 0 and run this script with -Name `"Full Name`"."
    }

    # The standalone Claude Code that mn calls in the background. Claude Desktop's own sign-in is not
    # shared with it, so it is installed here with the official installer when missing (a file, never piped).
    $ClaudeFallbacks = @(
        (Join-Path $UserHome '.local\bin\claude.exe'),
        (Join-Path $env:APPDATA 'npm\claude.cmd'),
        (Join-Path $UserHome '.claude\local\claude.exe'))
    $Claude = Find-Tool 'claude' $ClaudeFallbacks
    if (-not $Claude) {
        Write-Host 'Installing Claude Code (about 1 minute)...'
        $claudeInstaller = Join-Path $env:TEMP 'claude-install.ps1'
        Get-Download $ClaudeInstallUrl $claudeInstaller
        $ci = Invoke-Native 'powershell.exe' @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $claudeInstaller)
        Remove-Item -LiteralPath $claudeInstaller -Force -ErrorAction SilentlyContinue
        if ($ci.Code -ne 0) {
            $ciLast = [string]($ci.Lines | Select-Object -Last 1)
            Fail "Could not install Claude Code ($ciLast). Check the internet connection, then run this script again."
        }
        # The PATH of this session is not refreshed, so look in the usual folder too.
        $Claude = Find-Tool 'claude' $ClaudeFallbacks
        if (-not $Claude) {
            Fail "Claude Code was installed but was not found at $UserHome\.local\bin\claude.exe. Run this script again."
        }
    }
    $auth = Invoke-Native $Claude @('auth', 'status')
    $loggedIn = ($auth.Code -eq 0) -and (($auth.Lines -join ' ') -notmatch '"loggedIn"\s*:\s*false')
    if (-not $loggedIn) {
        if ($SkipPage) {
            Add-Warning 'Claude Code is not signed in. Do the sign-in step of SETUP.md (step 2) before the first recording.'
        } else {
            Fail "Claude Code is not signed in. Do the sign-in step of SETUP.md (step 2): run '$Claude auth login' (the user clicks Authorize in the browser), or have the user type 'claude' in PowerShell and sign in. Then run this script again."
        }
    }
    $Codex = Find-Tool 'codex' @((Join-Path $env:APPDATA 'npm\codex.cmd'))

    foreach ($d in @($ConfigDir, $DataDir, $BinDir)) {
        New-Item -ItemType Directory -Force -Path $d | Out-Null
    }

    # Keep the name and the notes folder of an earlier install when they are not given again.
    $OldNotesLine = ''
    $OldNameLine = ''
    if (Test-Path -LiteralPath $Config) {
        $oldLines = [System.IO.File]::ReadAllLines($Config)
        $OldNotesLine = [string]($oldLines | Where-Object { $_ -match '^notes_dir\s*=' } | Select-Object -First 1)
        $OldNameLine = [string]($oldLines | Where-Object { $_ -match '^my_name\s*=' } | Select-Object -First 1)
    }
    if ((-not $Name) -and (-not $OldNameLine) -and $Update) {
        Fail "The existing config at $Config has no name, so this cannot be an update. Do a normal install instead: follow SETUP.md from step 0 and run this script with -Name `"Full Name`"."
    }
    if ((-not $Name) -and (-not $OldNameLine)) {
        Fail 'The persons full name is missing. Ask the user for their full name and run this script again with -Name "Full Name".'
    }

    # ------------------------------------------------------------ 2. app
    Write-Step 'Installing the Meeting Notes app (about 1 minute)'
    $installer = Join-Path $DataDir 'MeetingNotes-windows-setup.exe'
    Get-Release 'MeetingNotes-windows-setup.exe' $installer
    try { Unblock-File -LiteralPath $installer } catch { }
    Stop-App (Find-AppExe)
    $inst = Start-Process -FilePath $installer -ArgumentList '/S' -Wait -PassThru
    if ($inst.ExitCode -ne 0) {
        Fail "The Meeting Notes installer failed (exit code $($inst.ExitCode)). If Windows showed a blue SmartScreen window, click More info, then Run anyway, and run this script again."
    }
    Remove-Item -LiteralPath $installer -Force -ErrorAction SilentlyContinue
    $AppExe = Find-AppExe
    if (-not $AppExe) {
        Fail 'The installer finished but Meeting Notes.exe was not found under %LOCALAPPDATA%. Show this message to the developer.'
    }
    try {
        Add-AppShortcuts $AppExe
    } catch {
        Add-Warning "Could not add the Meeting Notes icon to the Desktop ($($_.Exception.Message)). The user can open it from the Start menu."
    }

    # ------------------------------------------------------------ 3. mn
    Write-Step 'Installing the command-line tool (mn)'
    $mnNew = $Mn + '.new'
    Get-Release 'mn-windows.exe' $mnNew
    Move-Item -LiteralPath $mnNew -Destination $Mn -Force
    try { Unblock-File -LiteralPath $Mn } catch { }
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if (-not $userPath) { $userPath = '' }
    $parts = @($userPath.Split(';') | Where-Object { $_ })
    $onPath = $false
    foreach ($p in $parts) {
        if ($p.TrimEnd('\') -ieq $BinDir.TrimEnd('\')) { $onPath = $true }
    }
    if (-not $onPath) {
        [Environment]::SetEnvironmentVariable('Path', ((@($parts) + $BinDir) -join ';'), 'User')
        Write-Host "Added $BinDir to your PATH (new terminal windows will see it)."
    }
    $ver = Invoke-Native $Mn @('--version')
    if ($ver.Code -ne 0) { Fail 'The downloaded mn tool does not run on this computer.' }

    # ------------------------------------------------------------ 4. whisper
    Write-Step 'Installing whisper (the speech-to-text engine)'
    $WhisperDir = Join-Path $DataDir 'whisper'
    $Whisper = ''
    if ($WhisperCli) {
        if (-not (Test-Path -LiteralPath $WhisperCli)) { Fail "-WhisperCli $WhisperCli does not exist." }
        $Whisper = $WhisperCli
    } else {
        $existing = Get-ChildItem -LiteralPath $WhisperDir -Recurse -Filter 'whisper-cli.exe' -File -ErrorAction SilentlyContinue |
            Select-Object -First 1
        if ($existing) {
            $Whisper = [string]$existing.FullName
            Write-Host "Already installed: $Whisper"
        } else {
            $onPathWhisper = Find-Tool 'whisper-cli' @()
            if ($onPathWhisper) {
                $Whisper = $onPathWhisper
                Write-Host "Using the whisper-cli already on this computer: $Whisper"
            } else {
                $zip = Join-Path $DataDir 'whisper.zip'
                Write-Host 'Downloading whisper (about 15 MB)...'
                Get-Download $WhisperUrl $zip
                if (Test-Path -LiteralPath $WhisperDir) { Remove-Item -LiteralPath $WhisperDir -Recurse -Force }
                New-Item -ItemType Directory -Force -Path $WhisperDir | Out-Null
                Expand-Archive -LiteralPath $zip -DestinationPath $WhisperDir -Force
                Remove-Item -LiteralPath $zip -Force -ErrorAction SilentlyContinue
                $extracted = Get-ChildItem -LiteralPath $WhisperDir -Recurse -Filter 'whisper-cli.exe' -File | Select-Object -First 1
                if (-not $extracted) { Fail 'The whisper download did not contain whisper-cli.exe. Run this script again.' }
                $Whisper = [string]$extracted.FullName
            }
        }
    }
    $wcheck = Invoke-Native $Whisper @('--help')
    if ($wcheck.Code -ne 0) {
        Add-Warning "whisper-cli at $Whisper did not start. Windows may need the Microsoft Visual C++ Redistributable (search for it on microsoft.com and install it)."
    }

    # ------------------------------------------------------------ 5. model
    Write-Step 'Getting the speech model'
    if ($WhisperModel) {
        if (-not (Test-Path -LiteralPath $WhisperModel)) { Fail "-WhisperModel $WhisperModel does not exist." }
        $ModelPath = $WhisperModel
    } else {
        if ($Model -eq 'turbo') {
            $modelFile = 'ggml-large-v3-turbo-q5_0.bin'; $modelMin = 520000000; $modelMb = 574
        } else {
            $modelFile = 'ggml-small.bin'; $modelMin = 440000000; $modelMb = 488
        }
        $modelDir = Join-Path $DataDir 'models'
        New-Item -ItemType Directory -Force -Path $modelDir | Out-Null
        $ModelPath = Join-Path $modelDir $modelFile
        if ((Test-Path -LiteralPath $ModelPath) -and ((Get-Item -LiteralPath $ModelPath).Length -ge $modelMin)) {
            Write-Host "Already downloaded: $ModelPath"
        } else {
            Write-Host "Downloading $modelFile (about $modelMb MB, a few minutes)..."
            Get-Download ($ModelUrl + '/' + $modelFile) $ModelPath
            if ((Get-Item -LiteralPath $ModelPath).Length -lt $modelMin) {
                Fail 'The speech model download is incomplete. Run this script again.'
            }
        }
    }

    # Silero VAD (about 1 MB): whisper then skips silence, so it cannot invent lines there, and
    # "auto" picks the language from the first speech instead of from a silent start.
    $vadFile = 'ggml-silero-v5.1.2.bin'
    $vadSize = 885098
    $vadDir = Join-Path $DataDir 'models'
    New-Item -ItemType Directory -Force -Path $vadDir | Out-Null
    $VadPath = Join-Path $vadDir $vadFile
    if (-not ((Test-Path -LiteralPath $VadPath) -and ((Get-Item -LiteralPath $VadPath).Length -eq $vadSize))) {
        Get-Download ($VadUrl + '/' + $vadFile) $VadPath
        if ((Get-Item -LiteralPath $VadPath).Length -ne $vadSize) {
            Fail 'The VAD model download is incomplete. Run this script again.'
        }
    }

    # ------------------------------------------------------------ 6. config
    Write-Step 'Writing the config'
    if ($OldNotesLine) {
        $notesLine = $OldNotesLine
        $NotesDir = ($OldNotesLine -replace '^notes_dir\s*=\s*["'']', '') -replace '["'']\s*$', ''
    } else {
        $NotesDir = Join-Path $UserHome 'MeetingNotes'
        New-Item -ItemType Directory -Force -Path $NotesDir | Out-Null
        $notesLine = 'notes_dir = ' + (ConvertTo-TomlPath $NotesDir)
    }
    if ($Name) { $nameLine = 'my_name = ' + (ConvertTo-TomlString $Name) } else { $nameLine = $OldNameLine }

    $lines = New-Object System.Collections.ArrayList
    [void]$lines.Add('# Written by the Meeting Notes setup script. Re-running setup rewrites this file (the old one is kept as config.toml.bak).')
    [void]$lines.Add($notesLine)
    [void]$lines.Add('whisper_cli = ' + (ConvertTo-TomlPath $Whisper))
    [void]$lines.Add('whisper_model = ' + (ConvertTo-TomlPath $ModelPath))
    [void]$lines.Add('vad_model = ' + (ConvertTo-TomlPath $VadPath))
    [void]$lines.Add('claude_cli = ' + (ConvertTo-TomlPath $Claude))
    if ($Codex) { [void]$lines.Add('codex_cli = ' + (ConvertTo-TomlPath $Codex)) }
    if ($env:CLAUDE_CONFIG_DIR) { [void]$lines.Add('claude_config_dir = ' + (ConvertTo-TomlPath $env:CLAUDE_CONFIG_DIR)) }
    [void]$lines.Add($nameLine)
    [void]$lines.Add('language = "auto"')

    if (Test-Path -LiteralPath $Config) { Copy-Item -LiteralPath $Config -Destination ($Config + '.bak') -Force }
    # UTF-8 without a BOM: the Rust TOML parser rejects a BOM, and Set-Content -Encoding UTF8 writes one on 5.1.
    $utf8 = New-Object System.Text.UTF8Encoding($false)
    $configNew = $Config + '.new'
    [System.IO.File]::WriteAllText($configNew, (($lines.ToArray() -join "`n") + "`n"), $utf8)
    Move-Item -LiteralPath $configNew -Destination $Config -Force
    $cfgCheck = Invoke-Native $Mn @('config')
    if ($cfgCheck.Code -ne 0) {
        Fail "The config file at $Config is not valid. Show this message to the developer: $($cfgCheck.Lines -join ' ')"
    }

    # ------------------------------------------------------------ 7. page
    Write-Step 'Creating the My Meetings page (1 to 2 minutes)'
    $PageUrl = ''
    if ($SkipPage) {
        Write-Host 'Skipped (-SkipPage).'
    } else {
        $page = Invoke-Native $Mn @('init-page')
        $pageError = ''
        foreach ($l in $page.Lines) {
            $t = $l.Trim()
            if ($t.StartsWith('{')) {
                try {
                    $obj = ConvertFrom-Json -InputObject $t
                    if ($obj.state -eq 'published') { $PageUrl = [string]$obj.url }
                    if ($obj.state -eq 'error') { $pageError = [string]$obj.message }
                } catch { }
            } elseif ($t.StartsWith('error:')) {
                $pageError = $t
            }
        }
        if (($page.Code -ne 0) -or (-not $PageUrl)) {
            if (-not $pageError) { $pageError = 'no message' }
            Add-Warning "mn init-page failed: $pageError. Run '$Mn init-page' again and show this error to the developer."
            $PageUrl = ''
        }
    }
    if ((-not $PageUrl) -and (Test-Path -LiteralPath $UrlFile)) {
        $PageUrl = ([System.IO.File]::ReadAllLines($UrlFile) | Select-Object -First 1)
        if ($PageUrl) { $PageUrl = $PageUrl.Trim() }
    }

    # ------------------------------------------------------------ 8. launch
    Write-Step 'Starting the widget'
    $Launched = $false
    if ($SkipLaunch) {
        Write-Host 'Skipped (-SkipLaunch).'
    } else {
        Stop-App $AppExe
        Start-Process -FilePath $AppExe
        Start-Sleep -Seconds 4
        $procName = [IO.Path]::GetFileNameWithoutExtension($AppExe)
        if (Get-Process -Name $procName -ErrorAction SilentlyContinue) {
            $Launched = $true
            Write-Host 'The widget is running. It turns on start-at-login by itself on the first run.'
        } else {
            Add-Warning "The widget did not stay open. Start it from the Start menu (Meeting Notes) or run $AppExe."
        }
    }

    if ($PageUrl) { $pageValue = $PageUrl } else { $pageValue = $null }
    $result = [ordered]@{
        ok                = $true
        os                = 'windows'
        config            = $Config
        artifact_url_file = $UrlFile
        mn                = $Mn
        app               = $AppExe
        whisper_cli       = $Whisper
        whisper_model     = $ModelPath
        notes_dir         = $NotesDir
        claude_cli        = $Claude
        page_url          = $pageValue
        launched          = $Launched
        update            = [bool]$Update
        release           = $ReleaseLabel
        warnings         = @($script:Warnings.ToArray())
    }
    Write-Output ('SETUP_RESULT ' + (ConvertTo-AsciiJson $result))
} catch {
    Fail $_.Exception.Message
}
