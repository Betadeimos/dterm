# Builds dterm for the version in Cargo.toml into dist\: dterm-vX.Y.Z.exe, dterm-vX.Y.Z.zip
# with the exe, README and LICENSE, and SHA256SUMS.txt with the hashes of both.
#   .\release.ps1            build dist\ only
#   .\release.ps1 -Publish   also tag vX.Y.Z, push the tag and create the GitHub release
param([switch]$Publish)
# Cmdlets stop on errors; native tools are checked by exit code instead, since Windows PowerShell
# would turn cargo's progress on stderr into errors.
$PSDefaultParameterValues['*:ErrorAction'] = 'Stop'
Set-Location $PSScriptRoot

$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
$name = "dterm-v$version"
cargo build --release
if ($LASTEXITCODE) { throw 'cargo build failed' }

New-Item -ItemType Directory -Force dist | Out-Null
Copy-Item target\release\dterm.exe "dist\$name.exe"
$stage = "dist\$name"
New-Item -ItemType Directory -Force $stage | Out-Null
Copy-Item target\release\dterm.exe, README.md, LICENSE $stage
Compress-Archive -Force -Path "$stage\*" -DestinationPath "dist\$name.zip"
Remove-Item -Recurse $stage
# Hashes for checking a download against the release, in the format sha256sum -c reads.
$sums = "dist\SHA256SUMS.txt"
"$name.exe", "$name.zip" | ForEach-Object { "$((Get-FileHash "dist\$_" -Algorithm SHA256).Hash.ToLower())  $_" } | Set-Content -Encoding ascii $sums
Write-Host "built dist\$name.exe, dist\$name.zip and $sums"

if ($Publish) {
    if (git status --porcelain) { throw 'commit your changes first' }
    $gh = (Get-Command gh -ErrorAction SilentlyContinue).Source
    if (-not $gh) { $gh = "$env:ProgramFiles\GitHub CLI\gh.exe" } # the installed GitHub CLI
    git tag "v$version"
    git push origin "v$version"
    if ($LASTEXITCODE) { throw 'pushing the tag failed' }
    & $gh release create "v$version" "dist\$name.exe" "dist\$name.zip" $sums --title "dterm v$version" --generate-notes
}
