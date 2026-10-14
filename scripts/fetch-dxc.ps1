# Downloads the pinned DirectX Shader Compiler release into .tools/dxc
# (gitignored) and verifies its SHA-256. Lumen finds it through LUMEN_DXC:
#   $env:LUMEN_DXC = "$PWD\.tools\dxc\dxc.exe"
# Used by developers and by CI (shader runtime and scene golden tests).
$ErrorActionPreference = 'Stop'

$Tag = 'v1.9.2607'
$Asset = 'dxc_2026_07_29.zip'
$Sha256 = 'A1DFB116BA3EEAE6A1582291B53A8E7BF65AD760676BD3194685C8F7367CD241'

$Root = Split-Path -Parent $PSScriptRoot
$Dest = Join-Path $Root '.tools\dxc'
if (Test-Path (Join-Path $Dest 'dxc.exe')) {
    Write-Host "DXC already present in $Dest"
    exit 0
}

$Tmp = Join-Path ([IO.Path]::GetTempPath()) "lumen-dxc-$PID"
New-Item -ItemType Directory -Force $Tmp | Out-Null
try {
    $Zip = Join-Path $Tmp $Asset
    Invoke-WebRequest -UseBasicParsing -Uri "https://github.com/microsoft/DirectXShaderCompiler/releases/download/$Tag/$Asset" -OutFile $Zip
    $Actual = (Get-FileHash -Algorithm SHA256 $Zip).Hash
    if ($Actual -ne $Sha256) {
        throw "DXC download hash mismatch: $Actual"
    }
    Expand-Archive -Path $Zip -DestinationPath (Join-Path $Tmp 'x')
    New-Item -ItemType Directory -Force $Dest | Out-Null
    Copy-Item (Join-Path $Tmp 'x\bin\x64\*') $Dest
    Write-Host "DXC $Tag installed in $Dest"
} finally {
    Remove-Item -Recurse -Force $Tmp -ErrorAction SilentlyContinue
}
