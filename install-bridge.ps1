param([string]$Instance = $env:MC_INSTANCE)
$ErrorActionPreference = 'Stop'
if (-not $Instance -and (Test-Path -LiteralPath (Join-Path $PSScriptRoot '.env'))) {
    foreach ($minerLine in Get-Content -LiteralPath (Join-Path $PSScriptRoot '.env')) {
        if ($minerLine.Trim() -match '^MC_INSTANCE=(.*)$') { $Instance = $Matches[1].Trim().Trim("'", '"'); break }
    }
}
if (-not $Instance) { throw 'Set MC_INSTANCE in .env or pass -Instance with your Minecraft instance folder.' }
$minerInstance = (Resolve-Path -LiteralPath $Instance).Path
$minerMods = (Resolve-Path -LiteralPath (Join-Path $minerInstance 'mods')).Path
if (-not $minerMods.StartsWith($minerInstance.TrimEnd('\') + '\', [StringComparison]::OrdinalIgnoreCase)) {
    throw 'The mods folder must be inside the selected instance.'
}
$minerRunning = Get-CimInstance Win32_Process -Filter "Name = 'javaw.exe' OR Name = 'java.exe'" |
    Where-Object { $_.CommandLine -like '*--gameDir*' -and $_.CommandLine.Contains($minerInstance) }
if ($minerRunning) { throw 'Close this Minecraft instance before installing the bridge.' }
$minerVersion = (Get-Content (Join-Path $PSScriptRoot 'forge-bridge/gradle.properties') | Where-Object { $_ -match '^mod_version=' }) -replace '^mod_version=', ''
if ($minerVersion -notmatch '^\d+\.\d+\.\d+(?:[-+][A-Za-z0-9.-]+)?$') { throw 'Invalid bridge version.' }
$minerJarName = "flyminer-$minerVersion.jar"
$minerSource = Join-Path $PSScriptRoot "forge-bridge/build/libs/$minerJarName"
if (-not (Test-Path -LiteralPath $minerSource)) { throw "Build bridge $minerVersion first." }
$minerDestination = Join-Path $minerMods $minerJarName
$minerBackup = Join-Path $PSScriptRoot ('runtime\mod-backups\' + (Get-Date -Format 'yyyyMMdd-HHmmss'))
New-Item -ItemType Directory -Path $minerBackup -Force | Out-Null
$minerOld = @(Get-ChildItem -LiteralPath $minerMods -Filter 'flyminer-*.jar' | Where-Object { $_.Name -match '^flyminer-\d+\.\d+\.\d+(?:[-+][A-Za-z0-9.-]+)?\.jar$' })
foreach ($minerFile in $minerOld) {
    if ($minerFile.DirectoryName -ne $minerMods) { throw 'Unexpected bridge path.' }
    $minerCopy = Join-Path $minerBackup $minerFile.Name
    Copy-Item -LiteralPath $minerFile.FullName -Destination $minerCopy
    if ((Get-FileHash -LiteralPath $minerFile.FullName).Hash -ne (Get-FileHash -LiteralPath $minerCopy).Hash) { throw 'Backup verification failed.' }
}
$minerPending = $minerDestination + '.pending'
Copy-Item -LiteralPath $minerSource -Destination $minerPending -Force
if ((Get-FileHash -LiteralPath $minerSource).Hash -ne (Get-FileHash -LiteralPath $minerPending).Hash) { throw 'New bridge verification failed.' }
Move-Item -LiteralPath $minerPending -Destination $minerDestination -Force
foreach ($minerFile in $minerOld) {
    if ($minerFile.FullName -ne $minerDestination) { Remove-Item -LiteralPath $minerFile.FullName }
}
Get-FileHash -LiteralPath $minerDestination -Algorithm SHA256 | Format-List
Write-Output "Previous bridge saved in $minerBackup"
