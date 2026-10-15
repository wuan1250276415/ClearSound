# Downloads a static win64 GPL FFmpeg build and places the sidecar
# where Tauri's externalBin expects it:
#   src-tauri/binaries/ffmpeg-x86_64-pc-windows-msvc.exe
$ErrorActionPreference = "Stop"

$destDir = Join-Path $PSScriptRoot "..\src-tauri\binaries"
New-Item -ItemType Directory -Force -Path $destDir | Out-Null

$zip = Join-Path $env:TEMP "chengyin-ffmpeg-win64-gpl.zip"
$url = "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip"
Write-Host "Downloading $url"
Invoke-WebRequest -Uri $url -OutFile $zip

$extract = Join-Path $env:TEMP "chengyin-ffmpeg-win64-gpl"
if (Test-Path $extract) {
  Remove-Item -Recurse -Force $extract
}
Expand-Archive -Path $zip -DestinationPath $extract

$exe = Get-ChildItem -Path $extract -Recurse -Filter ffmpeg.exe | Select-Object -First 1
if (-not $exe) {
  throw "ffmpeg.exe was not found in the archive"
}

$target = Join-Path $destDir "ffmpeg-x86_64-pc-windows-msvc.exe"
Copy-Item $exe.FullName $target -Force
Write-Host "FFmpeg sidecar ready: $target"
