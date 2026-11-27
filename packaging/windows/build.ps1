param(
    [string]$OutputDirectory = (Join-Path $PSScriptRoot '..\..\dist')
)

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
if (-not [System.IO.Path]::IsPathFullyQualified($OutputDirectory)) {
    $OutputDirectory = Join-Path (Get-Location) $OutputDirectory
}
$OutputDirectory = [System.IO.Path]::GetFullPath($OutputDirectory)

$iscc = Get-Command ISCC.exe -ErrorAction SilentlyContinue
if ($iscc) {
    $compiler = $iscc.Source
} else {
    $compiler = Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6\ISCC.exe'
    if (-not (Test-Path -LiteralPath $compiler -PathType Leaf)) {
        throw 'Inno Setup 6 could not be found.'
    }
}

Push-Location (Join-Path $repo 'windows')
try {
    $metadataJson = cargo metadata --locked --no-deps --format-version 1
    if ($LASTEXITCODE -ne 0) { throw "cargo metadata failed ($LASTEXITCODE)" }
    $metadata = $metadataJson | ConvertFrom-Json
    $version = ($metadata.packages | Where-Object name -eq 'reimsbridge-windows').version
    if (-not $version) { throw 'Windows package version was not found.' }

    $target = 'x86_64-pc-windows-msvc'
    cargo build --locked --release --bins --target $target
    if ($LASTEXITCODE -ne 0) { throw "Windows release build failed ($LASTEXITCODE)" }
    $binaries = Join-Path $metadata.target_directory "$target\release"
} finally {
    Pop-Location
}

New-Item -ItemType Directory -Path $OutputDirectory -Force | Out-Null
$source = Join-Path $PSScriptRoot 'ReimsBridge.iss'
& $compiler "/DSourceDir=$binaries" "/DAppVersion=$version" "/O$OutputDirectory" $source
if ($LASTEXITCODE -ne 0) { throw "Inno Setup failed ($LASTEXITCODE)" }

$installer = Join-Path $OutputDirectory 'ReimsBridge-Windows-x64-Setup.exe'
if (-not (Test-Path -LiteralPath $installer -PathType Leaf)) {
    throw 'Inno Setup did not produce the expected installer.'
}
$signature = Get-AuthenticodeSignature -FilePath $installer
if ($signature.Status -ne 'NotSigned') {
    throw 'The Windows package must be unsigned.'
}
Get-FileHash -Algorithm SHA256 -LiteralPath $installer | Format-List
