# Builds Lumen's Windows installer (NSIS) into
# target/release/bundle/nsis/: lumen-ui with lumend, lumen-webhost, the
# `lumen` CLI and DXC beside it.
#
# With TAURI_SIGNING_PRIVATE_KEY (and _PASSWORD) set, it also writes the
# signed update files (`.nsis.zip` + `.sig`) the in-app updater installs;
# without them the installer is built alone (local test builds).
# Used by developers and by the release workflow.
$ErrorActionPreference = 'Stop'

# Runs a program, failing on its exit code only: Windows PowerShell turns a
# program's stderr (cargo's progress) into errors when output is redirected.
function Invoke-Native([string]$What, [scriptblock]$Command) {
    $ErrorActionPreference = 'Continue'
    & $Command
    if ($LASTEXITCODE) { throw "$What failed (exit $LASTEXITCODE)" }
}

$Root = Split-Path -Parent $PSScriptRoot
$Ui = Join-Path $Root 'apps\lumen-ui'
$Bins = Join-Path $Ui 'src-tauri\binaries'

& (Join-Path $PSScriptRoot 'fetch-dxc.ps1')

Push-Location $Root
try {
    Invoke-Native 'cargo build' { cargo build --release --locked -p lumend -p lumen-webhost -p lumen-cli }

    # Tauri's externalBin wants each program named for its target triple.
    $Triple = ((rustc -vV | Select-String '^host: ').Line -replace '^host: ', '').Trim()
    New-Item -ItemType Directory -Force $Bins | Out-Null
    foreach ($Name in 'lumend', 'lumen-webhost', 'lumen') {
        Copy-Item (Join-Path $Root "target\release\$Name.exe") (Join-Path $Bins "$Name-$Triple.exe")
    }

    Set-Location $Ui
    # CI installs exactly the lock file; locally keep the existing install.
    if ($env:CI) { Invoke-Native 'npm ci' { npm ci } } else { Invoke-Native 'npm install' { npm install } }
    $Config = @('--config', 'src-tauri/tauri.bundle.conf.json')
    if (-not $env:TAURI_SIGNING_PRIVATE_KEY) {
        Write-Host 'TAURI_SIGNING_PRIVATE_KEY not set: building without update files'
        # A file: Windows PowerShell mangles JSON quotes passed to programs.
        $NoUpdates = Join-Path ([IO.Path]::GetTempPath()) 'lumen-no-updater.json'
        Set-Content -Encoding ascii $NoUpdates '{"bundle":{"createUpdaterArtifacts":false}}'
        $Config += @('--config', $NoUpdates)
    }
    # Old installers left here get scanned by antivirus on rebuild, which
    # can lock them while the bundler writes (os error 32).
    Remove-Item -Recurse -Force (Join-Path $Root 'target\release\bundle\nsis') -ErrorAction SilentlyContinue
    Invoke-Native 'tauri build' { npx tauri build @Config }
} finally {
    Pop-Location
}
Get-ChildItem (Join-Path $Root 'target\release\bundle\nsis') | Select-Object Name, Length
