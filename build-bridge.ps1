param(
    [string]$JdkPath = $env:JAVA_HOME,
    [switch]$Offline
)
$ErrorActionPreference = 'Stop'
$miningJava = if ($JdkPath) { Join-Path $JdkPath 'bin\java.exe' } else { (Get-Command java -ErrorAction Stop).Source }
if (-not (Test-Path -LiteralPath $miningJava)) { throw 'Set JAVA_HOME or provide a JDK 17 location using -JdkPath.' }
$miningJavaVersion = (& $miningJava -version 2>&1 | Out-String)
if ($LASTEXITCODE -ne 0 -or $miningJavaVersion -notmatch 'version "17[.\"]') {
    throw 'The bridge requires JDK 17. Set JAVA_HOME or pass -JdkPath.'
}
$miningGradleHome = Join-Path $PSScriptRoot '.gradle-home'
# OpenJDK falls back to TCP if its AF_UNIX listener cannot bind here.
# MSIX-launched Java can otherwise fail inside UnixDomainSockets.connect0.
$miningSocketPath = Join-Path $miningGradleHome 'no-af-unix'
if (Test-Path -LiteralPath $miningSocketPath) { throw 'The build fallback path no-af-unix must not exist.' }
$miningPreviousJavaOptions = $env:JAVA_TOOL_OPTIONS
try {
    $env:JAVA_TOOL_OPTIONS = ($miningPreviousJavaOptions + ' "-Djdk.net.unixdomain.tmpdir=' + $miningSocketPath + '"').Trim()
    Push-Location (Join-Path $PSScriptRoot 'forge-bridge')
    try {
        $miningExtraArgs = @()
        if ($Offline) { $miningExtraArgs += '--offline' }
        & $miningJava -classpath 'gradle\wrapper\gradle-wrapper.jar' org.gradle.wrapper.GradleWrapperMain --gradle-user-home $miningGradleHome build --no-daemon --console=plain @miningExtraArgs
        if ($LASTEXITCODE -ne 0) { throw "Forge build failed with exit code $LASTEXITCODE." }
    } finally { Pop-Location }
} finally { $env:JAVA_TOOL_OPTIONS = $miningPreviousJavaOptions }
