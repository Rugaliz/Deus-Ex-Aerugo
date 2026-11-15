<#
.SYNOPSIS
  Builds the Windows executable deusex.exe on Windows into .\dist (SDL3 compiled from source and linked in).

.DESCRIPTION
  Requirements:
    - Rust (https://rustup.rs, the default x86_64-pc-windows-msvc toolchain)
    - Visual Studio 2022 or its Build Tools with "Desktop development with C++" (includes CMake, which builds SDL3)
    - The Vulkan SDK (https://vulkan.lunarg.com) for glslc, the shader compiler; the script finds it through VULKAN_SDK
  Run it from a "Developer PowerShell for VS 2022", or any PowerShell where cargo, cmake and glslc are found.

.EXAMPLE
  .\scripts\build-windows.ps1
  .\scripts\build-windows.ps1 -Install "C:\GOG Games\Deus Ex GOTY"
  .\scripts\build-windows.ps1 -Run "C:\Program Files (x86)\Steam\steamapps\common\Deus Ex"
#>
param(
    # copy deusex.exe into this Deus Ex folder (the one with System\ and Maps\)
    [string]$Install,
    # copy it there and start it
    [string]$Run,
    # remove earlier build output first
    [switch]$Clean
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$dir = if ($Run) { $Run } else { $Install }
if ($dir -and -not ((Test-Path (Join-Path $dir 'System')) -and (Test-Path (Join-Path $dir 'Maps')))) {
    throw "'$dir' does not look like a Deus Ex folder (no System\ and Maps\)"
}

# glslc comes with the Vulkan SDK
if (-not (Get-Command glslc -ErrorAction SilentlyContinue) -and $env:VULKAN_SDK) {
    $env:PATH = (Join-Path $env:VULKAN_SDK 'Bin') + ';' + $env:PATH
}
# CMake comes with Visual Studio; find it through vswhere when it is not on PATH
if (-not (Get-Command cmake -ErrorAction SilentlyContinue)) {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (Test-Path $vswhere) {
        $vs = & $vswhere -latest -products * -property installationPath
        $cm = Join-Path $vs 'Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin'
        if (Test-Path $cm) { $env:PATH = $cm + ';' + $env:PATH }
    }
}
$missing = @()
foreach ($tool in 'cargo', 'cmake', 'glslc') {
    if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) { $missing += $tool }
}
if ($missing) { throw "missing tools: $($missing -join ', ') (see Get-Help .\scripts\build-windows.ps1)" }

if ($Clean) {
    Write-Host '== cleaning earlier build output'
    cargo clean
    if ($LASTEXITCODE) { exit $LASTEXITCODE }
}
Write-Host '== building (dist profile)'
cargo build --locked --profile dist -p dx-play --features static-sdl
if ($LASTEXITCODE) { exit $LASTEXITCODE }

New-Item -ItemType Directory -Force -Path dist | Out-Null
Copy-Item -Force target\dist\dx-play.exe dist\deusex.exe
$size = [math]::Round((Get-Item dist\deusex.exe).Length / 1MB, 1)
Write-Host "== built: $root\dist\deusex.exe ($size MB)"

if ($dir) {
    Copy-Item -Force dist\deusex.exe (Join-Path $dir 'deusex.exe')
    Write-Host "== installed: $(Join-Path $dir 'deusex.exe')"
    if ($Run) { Start-Process -FilePath (Join-Path $dir 'deusex.exe') -WorkingDirectory $dir }
}
