# Install mx-keyboard-switcher from a GitHub Release, falling back to a
# local cargo build when no matching prebuilt asset exists.
#Requires -Version 5.1
[CmdletBinding()]
param(
    [switch]$NoAutostart,
    [switch]$FromSource,
    [string]$Version
)

$ErrorActionPreference = 'Stop'

$Repo = 'itmaxk/mx-keyboard-switcher'
$BinName = 'mx-keyboard-switcher.exe'
$InstallDir = Join-Path $env:LOCALAPPDATA 'Programs\mx-keyboard-switcher'
$Triplet = 'x86_64-pc-windows-msvc'
$RunValueName = 'MXKeyboardSwitcher'

function Write-Info([string]$Message) {
    Write-Host $Message
}

function Write-Warn([string]$Message) {
    Write-Warning $Message
}

function Get-LatestTag {
    $uri = "https://api.github.com/repos/$Repo/releases/latest"
    $release = Invoke-RestMethod -Uri $uri -Headers @{ 'User-Agent' = 'mx-keyboard-switcher-install' }
    if (-not $release.tag_name) {
        throw 'latest release has no tag_name'
    }
    return [string]$release.tag_name
}

function Get-PrebuiltBinary {
    param([Parameter(Mandatory = $true)][string]$Tag)

    $archive = "mx-keyboard-switcher-$Triplet.zip"
    $url = "https://github.com/$Repo/releases/download/$Tag/$archive"
    $tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("mxks-install-" + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $tmp | Out-Null

    try {
        Write-Info "Downloading prebuilt binary: $url"
        $zipPath = Join-Path $tmp $archive
        Invoke-WebRequest -Uri $url -OutFile $zipPath -UseBasicParsing
        Expand-Archive -Path $zipPath -DestinationPath $tmp -Force
        $exe = Join-Path $tmp $BinName
        if (-not (Test-Path -LiteralPath $exe)) {
            throw "archive does not contain $BinName"
        }
        return @{ Path = $exe; TempDir = $tmp }
    }
    catch {
        if (Test-Path -LiteralPath $tmp) {
            Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
        }
        throw
    }
}

function Build-FromSource {
    $cargo = Get-Command cargo -ErrorAction SilentlyContinue
    if (-not $cargo) {
        throw 'cargo not found. Install Rust from https://rustup.rs and re-run.'
    }

    $git = Get-Command git -ErrorAction SilentlyContinue
    if (-not $git) {
        throw 'git not found. Install Git and re-run.'
    }

    $localManifest = Join-Path (Get-Location) 'crates\mxks-app\Cargo.toml'
    if (Test-Path -LiteralPath $localManifest) {
        Write-Info 'Building from local checkout...'
        & cargo build --release -p mxks-app
        if ($LASTEXITCODE -ne 0) {
            throw "cargo build failed with exit code $LASTEXITCODE"
        }
        $exe = Join-Path (Get-Location) "target\release\$BinName"
        if (-not (Test-Path -LiteralPath $exe)) {
            throw "built binary not found: $exe"
        }
        return @{ Path = $exe; TempDir = $null }
    }

    $tmp = Join-Path $env:TEMP ("mxks-src-" + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $tmp | Out-Null
    try {
        Write-Info "Cloning $Repo..."
        & git clone --depth 1 "https://github.com/$Repo.git" (Join-Path $tmp 'src')
        if ($LASTEXITCODE -ne 0) {
            throw "git clone failed with exit code $LASTEXITCODE"
        }
        Push-Location (Join-Path $tmp 'src')
        try {
            & cargo build --release -p mxks-app
            if ($LASTEXITCODE -ne 0) {
                throw "cargo build failed with exit code $LASTEXITCODE"
            }
        }
        finally {
            Pop-Location
        }
        $exe = Join-Path $tmp "src\target\release\$BinName"
        if (-not (Test-Path -LiteralPath $exe)) {
            throw "built binary not found: $exe"
        }
        return @{ Path = $exe; TempDir = $tmp }
    }
    catch {
        if (Test-Path -LiteralPath $tmp) {
            Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
        }
        throw
    }
}

function Install-Binary {
    param([Parameter(Mandatory = $true)][string]$SourcePath)

    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    Stop-Process -Name 'mx-keyboard-switcher' -ErrorAction SilentlyContinue
    Start-Sleep -Milliseconds 200

    $dest = Join-Path $InstallDir $BinName
    Copy-Item -LiteralPath $SourcePath -Destination $dest -Force
    Write-Info "Installed $dest"

    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if (-not $userPath) {
        $userPath = ''
    }
    $parts = @($userPath -split ';' | Where-Object { $_ -ne '' })
    if ($parts -notcontains $InstallDir) {
        $newPath = if ($userPath.Trim().Length -eq 0) { $InstallDir } else { "$userPath;$InstallDir" }
        [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
        $env:Path = "$env:Path;$InstallDir"
        Write-Info "Added $InstallDir to user PATH."
    }
}

function Enable-Autostart {
    $exe = Join-Path $InstallDir $BinName
    $value = '"' + $exe + '"'
    $runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
    Set-ItemProperty -Path $runKey -Name $RunValueName -Value $value
    Write-Info "Autostart registry value: $runKey\$RunValueName"
    Start-Process -FilePath $exe
    Write-Info "Started $BinName."
}

function Show-Done {
    $exe = Join-Path $InstallDir $BinName
    Write-Info ''
    Write-Info 'Done.'
    Write-Info "  Binary:  $exe"
    Write-Info "  Run:     $exe"
    if (-not $NoAutostart) {
        Write-Info '  Disable autostart:'
        Write-Info "    Remove-ItemProperty -Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -Name $RunValueName"
    }
}

$tempToClean = @()
try {
    $sourcePath = $null
    $useSource = [bool]$FromSource

    if (-not $useSource) {
        try {
            if (-not $Version) {
                $Version = Get-LatestTag
            }
            $prebuilt = Get-PrebuiltBinary -Tag $Version
            $sourcePath = $prebuilt.Path
            if ($prebuilt.TempDir) {
                $tempToClean += $prebuilt.TempDir
            }
        }
        catch {
            Write-Warn "prebuilt download failed ($($_.Exception.Message)); falling back to source build"
            $useSource = $true
        }
    }

    if ($useSource) {
        $built = Build-FromSource
        $sourcePath = $built.Path
        if ($built.TempDir) {
            $tempToClean += $built.TempDir
        }
    }

    Install-Binary -SourcePath $sourcePath

    if ($NoAutostart) {
        Write-Info 'Skipped autostart (-NoAutostart).'
    }
    else {
        Enable-Autostart
    }

    Show-Done
}
finally {
    foreach ($dir in $tempToClean) {
        if ($dir -and (Test-Path -LiteralPath $dir)) {
            Remove-Item -LiteralPath $dir -Recurse -Force -ErrorAction SilentlyContinue
        }
    }
}
