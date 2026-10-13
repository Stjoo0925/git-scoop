param(
    [ValidateSet('check', 'test', 'run', 'lint', 'fixtures', 'benchmark')]
    [string]$Action = 'check',
    [string]$Destination
)

$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$cargoBinary = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
if (-not (Test-Path -LiteralPath $cargoBinary)) {
    throw 'The Rust toolchain is not installed for this user.'
}
$vsLocator = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
if (-not (Test-Path -LiteralPath $vsLocator)) {
    throw 'Install the Visual Studio C++ build tools.'
}
$vsInstall = & $vsLocator -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if (-not $vsInstall) {
    throw 'Install the Visual Studio C++ x64 compiler.'
}

# Configure MSVC for this session without changing the global PATH.
& (Join-Path $vsInstall 'Common7\Tools\Launch-VsDevShell.ps1') -Arch amd64 -HostArch amd64 -SkipAutomaticLocation | Out-Null
$env:PATH = (Split-Path -Parent $cargoBinary) + [IO.Path]::PathSeparator + $env:PATH
Push-Location -LiteralPath $projectRoot
try {
    switch ($Action) {
        'check' { & $cargoBinary check --workspace --locked }
        'test' { & $cargoBinary test --workspace --locked }
        'run' { & $cargoBinary run -p git-scoop --locked }
        'lint' { & $cargoBinary clippy --workspace --all-targets --locked -- -D warnings }
        'fixtures' {
            if (-not $Destination) { throw 'Supply a new output directory with -Destination.' }
            & $cargoBinary run -p git-scoop-fixtures --locked -- $Destination
        }
        'benchmark' {
            if (-not $Destination) { throw 'Supply a new output directory with -Destination.' }
            & $cargoBinary run -p git-scoop-fixtures --locked -- $Destination --benchmark
        }
    }
    if ($LASTEXITCODE -ne 0) { throw "Development command failed: $Action" }
}
finally {
    Pop-Location
}
