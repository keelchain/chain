# Install the Keel command-line tools on Windows (keel, keel-node,
# keel-indexer, keel-observer). keel-tss is not built for Windows: run it
# under WSL with the Linux install.
#
#   irm https://keelchain.com/install.ps1 | iex
#
# $env:KEEL_INSTALL_DIR  where the programs go (default: %LOCALAPPDATA%\Keel\bin)
$ErrorActionPreference = 'Stop'
$base = if ($env:KEEL_DOWNLOADS) { $env:KEEL_DOWNLOADS } else { 'https://keelchain.com/downloads' }
$dir = if ($env:KEEL_INSTALL_DIR) { $env:KEEL_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Keel\bin' }
$name = 'keel-windows-x86_64'
$tmp = Join-Path ([IO.Path]::GetTempPath()) ("keel-" + [Guid]::NewGuid())
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
  Write-Host "Downloading $name.zip"
  Invoke-WebRequest "$base/$name.zip" -OutFile "$tmp\$name.zip" -UseBasicParsing
  Invoke-WebRequest "$base/SHA256SUMS" -OutFile "$tmp\SHA256SUMS" -UseBasicParsing
  $want = (Get-Content "$tmp\SHA256SUMS" | Where-Object { $_ -match " $name\.zip$" }) -split ' ' | Select-Object -First 1
  $got = (Get-FileHash "$tmp\$name.zip" -Algorithm SHA256).Hash.ToLower()
  if (-not $want -or $want -ne $got) { throw "Checksum mismatch for $name.zip: nothing was installed." }
  Expand-Archive "$tmp\$name.zip" -DestinationPath $tmp
  New-Item -ItemType Directory -Force -Path $dir | Out-Null
  Copy-Item "$tmp\$name\*.exe" $dir -Force
  Write-Host "Installed in ${dir}:" ((Get-ChildItem "$tmp\$name\*.exe").Name -join ' ')
  & "$dir\keel.exe" --version
  $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
  if (($userPath -split ';') -notcontains $dir) {
    [Environment]::SetEnvironmentVariable('Path', "$userPath;$dir", 'User')
    Write-Host "Added $dir to your PATH. Open a new terminal to use it."
  }
} finally {
  Remove-Item -Recurse -Force $tmp
}
