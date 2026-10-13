param(
    [switch]$SkipBuild,
    [switch]$TestMode,
    [string]$OutputDirectory,
    [string]$Compiler = "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe"
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$projectRoot = Split-Path -Parent $PSScriptRoot
$target = 'x86_64-pc-windows-msvc'
$cargo = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
if (-not $OutputDirectory) { $OutputDirectory = Join-Path $projectRoot '.local\release' }
$OutputDirectory = [IO.Path]::GetFullPath($OutputDirectory)
$compilerDirectory = Split-Path -Parent $Compiler
$pins = @{
    'ISCC.exe' = 'EB6F4410C8DB367A5F74127E8025AD2CCACC0AFABBE783959D237DF3050F97FB'
    'ISCmplr.dll' = '1A6FF68241E7F3B18543BB875283788913ABBDD7B9D7403AC094598D044DEF7A'
    'ISPP.dll' = '903729FCE54ECE07623C8EB795320614929AD2693D3B814E3DA0AB23F9D494D2'
}
foreach ($name in $pins.Keys) {
    $path = Join-Path $compilerDirectory $name
    if (-not (Test-Path -LiteralPath $path) -or (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash -ne $pins[$name]) {
        throw "Compiler pin mismatch: $name. Use the verified Inno Setup 6.7.1 toolchain."
    }
}
$vsLocator = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
$vsInstall = & $vsLocator -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if (-not $vsInstall) { throw 'Install the Visual Studio C++ x64 build tools.' }
& (Join-Path $vsInstall 'Common7\Tools\Launch-VsDevShell.ps1') -Arch amd64 -HostArch amd64 -SkipAutomaticLocation | Out-Null
Push-Location -LiteralPath $projectRoot
$previousFlags = $env:RUSTFLAGS
try {
    if (-not $SkipBuild) {
        $env:RUSTFLAGS = '-C target-feature=+crt-static'
        & $cargo build --release --target $target -p git-scoop --locked
        if ($LASTEXITCODE -ne 0) { throw 'Release build failed.' }
    }
    $executable = Join-Path $projectRoot "target\$target\release\git-scoop.exe"
    if (-not (Test-Path -LiteralPath $executable)) { throw 'The release executable is missing.' }
    $headers = & dumpbin.exe /headers $executable
    if ($LASTEXITCODE -ne 0 -or -not ($headers -match '8664 machine')) { throw 'The executable must be an x64 PE image.' }
    $dependencies = & dumpbin.exe /dependents $executable
    if ($LASTEXITCODE -ne 0) { throw 'Could not inspect executable imports.' }
    if ($dependencies -match '(?i)(vcruntime|msvcp|msvcr|concrt|ucrtbase|api-ms-win-crt).*\.dll') {
        throw 'Dynamic CRT dependency detected. Rebuild with static CRT before packaging.'
    }
    $payload = Join-Path $projectRoot '.local\release\payload'
    New-Item -ItemType Directory -Path $payload, $OutputDirectory -Force | Out-Null
    Copy-Item -LiteralPath $executable -Destination (Join-Path $payload 'git-scoop.exe')
    Copy-Item -LiteralPath (Join-Path $projectRoot 'LICENSE') -Destination (Join-Path $payload 'LICENSE.txt')
    Copy-Item -LiteralPath (Join-Path $projectRoot 'packaging\INSTALLATION.txt') -Destination $payload
    $json = & $cargo metadata --locked --offline --format-version 1 --filter-platform $target
    if ($LASTEXITCODE -ne 0) { throw 'Offline locked dependency metadata is unavailable.' }
    $metadata = ($json -join "`n") | ConvertFrom-Json
    $licenseRoot = Join-Path $projectRoot 'packaging\licenses'
    $provenance = @(Get-Content -LiteralPath (Join-Path $licenseRoot 'provenance.json') -Raw | ConvertFrom-Json)
    $app = @($metadata.packages | Where-Object name -eq 'git-scoop')[0]
    $nodes = @{}; foreach ($node in $metadata.resolve.nodes) { $nodes[$node.id] = $node }
    $selected = [Collections.Generic.HashSet[string]]::new()
    $pending = [Collections.Generic.Queue[string]]::new(); $pending.Enqueue($app.id)
    while ($pending.Count) {
        $id = $pending.Dequeue()
        if (-not $selected.Add($id)) { continue }
        foreach ($dep in $nodes[$id].deps) {
            if (@($dep.dep_kinds | Where-Object { $_.kind -ne 'dev' }).Count) { $pending.Enqueue($dep.pkg) }
        }
    }
    $notice = [Text.StringBuilder]::new()
    [void]$notice.AppendLine('Git Scoop dependency license notices (locked Windows x64 application dependency closure).')
    foreach ($package in @($metadata.packages | Where-Object { $selected.Contains($_.id) -and $_.source } | Sort-Object name, version)) {
        $sourceDirectory = Split-Path -Parent $package.manifest_path
        [void]$notice.AppendLine("`n==== $($package.name) $($package.version) ====")
        [void]$notice.AppendLine("Declared license: $($package.license)")
        [void]$notice.AppendLine("Source: $($package.source)")
        $licensePaths = [Collections.Generic.HashSet[string]]::new()
        foreach ($file in Get-ChildItem -LiteralPath $sourceDirectory -Recurse -File | Where-Object { $_.Name -match '^(?i:LICENSE|LICENCE|COPYING|NOTICE|COPYRIGHT)([.\-_]|$)' }) {
            [void]$licensePaths.Add($file.FullName)
        }
        if ($package.license_file) { [void]$licensePaths.Add((Join-Path $sourceDirectory $package.license_file)) }
        $vendored = @($provenance | Where-Object { $_.package -eq $package.name -and $_.version -eq $package.version })
        foreach ($entry in $vendored) {
            $vcs = Get-Content -LiteralPath (Join-Path $sourceDirectory '.cargo_vcs_info.json') -Raw | ConvertFrom-Json
            if ($entry.source -notmatch [regex]::Escape('/' + $vcs.git.sha1 + '/')) { throw "License source commit mismatch: $($package.name)." }
            $path = [IO.Path]::GetFullPath((Join-Path $licenseRoot $entry.file))
            if (-not $path.StartsWith($licenseRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) { throw 'License path escapes its directory.' }
            if (-not (Test-Path -LiteralPath $path) -or (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash -ne $entry.sha256) { throw "Vendored license hash mismatch: $($entry.file)." }
            [void]$licensePaths.Add($path)
        }
        if ($package.name -eq 'epaint_default_fonts') {
            foreach ($required in 'OFL.txt', 'UFL.txt', 'Hack-Regular.txt', 'emoji-icon-font-mit-license.txt') {
                if (-not @($vendored | Where-Object { $_.file -eq "epaint_default_fonts/$required" }).Count) { throw "Embedded font notice missing: $required." }
            }
        }
        if (-not $licensePaths.Count) { throw "License text is missing for $($package.name). Inspect its cached source before release." }
        foreach ($path in @($licensePaths | Sort-Object)) {
            [void]$notice.AppendLine("--- $([IO.Path]::GetRelativePath($sourceDirectory, $path)) ---")
            [void]$notice.AppendLine([IO.File]::ReadAllText($path))
        }
    }
    $sysroot = & (Join-Path (Split-Path -Parent $cargo) 'rustc.exe') --print sysroot
    if ($LASTEXITCODE -ne 0) { throw 'Rust toolchain license location is unavailable.' }
    $rustDocumentation = Join-Path $sysroot 'share\doc\rust'
    $rustCopyright = Join-Path $rustDocumentation 'COPYRIGHT-library.html'
    if (-not (Test-Path -LiteralPath $rustCopyright)) { throw 'Rust standard library copyright notices are missing.' }
    [void]$notice.AppendLine("`n==== Rust standard library ====")
    [void]$notice.AppendLine([IO.File]::ReadAllText($rustCopyright))
    foreach ($file in Get-ChildItem -LiteralPath (Join-Path $rustDocumentation 'licenses') -File | Sort-Object Name) {
        [void]$notice.AppendLine("--- $($file.Name) ---")
        [void]$notice.AppendLine([IO.File]::ReadAllText($file.FullName))
    }
    Copy-Item -LiteralPath (Join-Path $compilerDirectory 'license.txt') -Destination (Join-Path $payload 'INNO-SETUP-LICENSE.txt')
    [void]$notice.AppendLine("`n==== Inno Setup installer runtime ====")
    [void]$notice.AppendLine([IO.File]::ReadAllText((Join-Path $compilerDirectory 'license.txt')))
    [IO.File]::WriteAllText((Join-Path $payload 'THIRD-PARTY-NOTICES.txt'), $notice.ToString(), [Text.UTF8Encoding]::new($false))
    $compilerArguments = @('/Qp', "/DPayloadDir=$payload", "/DAppVersion=$($app.version)", "/O$OutputDirectory")
    if ($TestMode) { $compilerArguments += '/DTestMode=1' }
    & $Compiler @compilerArguments (Join-Path $projectRoot 'packaging\git-scoop.iss')
    if ($LASTEXITCODE -ne 0) { throw 'Installer compilation failed.' }
    $suffix = if ($TestMode) { '-validation' } else { '' }
    $installer = Join-Path $OutputDirectory "GitScoop-$($app.version)-windows-x64-setup$suffix.exe"
    $manifest = [ordered]@{
        version = $app.version; target = $target; compiler = 'Inno Setup 6.7.1'; compilerHashes = $pins
        cargoLockSha256 = (Get-FileHash -LiteralPath 'Cargo.lock' -Algorithm SHA256).Hash
        executableSha256 = (Get-FileHash -LiteralPath $executable -Algorithm SHA256).Hash
        installerSha256 = (Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash
        imports = @($dependencies | Where-Object { $_ -match '\.dll' } | ForEach-Object { $_.Trim() })
        licensePackageCount = @($metadata.packages | Where-Object { $selected.Contains($_.id) -and $_.source }).Count
        validationIdentity = [bool]$TestMode
        installVerification = 'Not performed by package.ps1; verify install, launch, restart and uninstall separately.'
    }
    $manifest | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $OutputDirectory 'package-manifest.json') -Encoding utf8
    Write-Output $installer
}
finally {
    $env:RUSTFLAGS = $previousFlags
    Pop-Location
}
