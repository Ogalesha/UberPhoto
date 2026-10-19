# Crea lo ZIP portabile di UberPhoto per Windows con ExifTool incluso.
# Uso: pwsh -File packaging/windows/package.ps1 [-Version 0.3.0]
param(
    [string]$Version = "",
    [string]$ExifToolVersion = "13.59"
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"
$Root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$Bin = Join-Path $Root "target\release\uberphoto-app.exe"
if (-not (Test-Path $Bin)) {
    throw "binario non trovato: $Bin (esegui prima cargo build --release -p uberphoto-app)"
}

if (-not $Version) {
    $Version = (Select-String -Path (Join-Path $Root "Cargo.toml") -Pattern '^version = "(.*)"$').Matches[0].Groups[1].Value
}

$Cache = Join-Path $Root "target\exiftool-cache"
$Zip = Join-Path $Cache "exiftool-$ExifToolVersion`_64.zip"
$Extract = Join-Path $Cache "exiftool-$ExifToolVersion`_64"
if (-not (Test-Path $Zip)) {
    New-Item -ItemType Directory -Force -Path $Cache | Out-Null
    $url = "https://sourceforge.net/projects/exiftool/files/exiftool-$ExifToolVersion`_64.zip/download"
    Write-Host "Scarico ExifTool $ExifToolVersion da $url"
    & curl.exe -fL --retry 5 --retry-all-errors --connect-timeout 30 -o $Zip $url
    if ($LASTEXITCODE -ne 0) {
        throw "download di ExifTool fallito (curl codice $LASTEXITCODE)"
    }
}
if (-not (Test-Path $Zip) -or (Get-Item $Zip).Length -lt 1000000) {
    throw "archivio ExifTool non valido: $Zip"
}
if (-not (Test-Path (Join-Path $Extract "exiftool(-k).exe"))) {
    Write-Host "Estraggo $Zip in $Cache"
    Expand-Archive -Path $Zip -DestinationPath $Cache -Force
}
if (-not (Test-Path (Join-Path $Extract "exiftool(-k).exe"))) {
    throw "contenuto di ExifTool inatteso in $Extract"
}

$Stage = Join-Path $Root "target\windows-package"
if (Test-Path $Stage) { Remove-Item -Recurse -Force $Stage }
New-Item -ItemType Directory -Force -Path $Stage | Out-Null
Copy-Item $Bin (Join-Path $Stage "uberphoto.exe")
Copy-Item (Join-Path $Extract "exiftool(-k).exe") (Join-Path $Stage "exiftool.exe")
Copy-Item -Recurse (Join-Path $Extract "exiftool_files") (Join-Path $Stage "exiftool_files")
Copy-Item (Join-Path $Extract "README.txt") (Join-Path $Stage "EXIFTOOL-README.txt")

$Archive = Join-Path $Root "target\uberphoto-windows-x86_64.zip"
if (Test-Path $Archive) { Remove-Item -Force $Archive }
Compress-Archive -Path (Join-Path $Stage "*") -DestinationPath $Archive
Write-Output $Archive
