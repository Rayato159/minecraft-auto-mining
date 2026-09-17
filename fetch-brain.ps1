param([string]$Destination = (Join-Path $PSScriptRoot 'references/Drosophila_brain_model'))
$ErrorActionPreference = 'Stop'
function Get-BrainChecksum([string]$Path) {
    # A Windows Git checkout can translate the upstream CSV's LF to CRLF.
    if ([IO.Path]::GetExtension($Path) -eq '.csv' -or $Path.EndsWith('.csv.download')) {
        $brainText = [IO.File]::ReadAllText($Path).Replace("`r`n", "`n")
        $brainHasher = [Security.Cryptography.SHA256]::Create()
        try { return ([BitConverter]::ToString($brainHasher.ComputeHash([Text.Encoding]::UTF8.GetBytes($brainText)))).Replace('-', '') }
        finally { $brainHasher.Dispose() }
    }
    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash
}
$brainManifest = Get-Content -Raw -LiteralPath (Join-Path $PSScriptRoot 'data/flywire-v783.json') | ConvertFrom-Json
New-Item -ItemType Directory -Path $Destination -Force | Out-Null
foreach ($brainFile in $brainManifest.files) {
    $brainTarget = Join-Path $Destination $brainFile.name
    if (Test-Path -LiteralPath $brainTarget) {
        if ((Get-BrainChecksum $brainTarget) -ne $brainFile.sha256) {
            throw "Existing $($brainFile.name) differs from the pinned data. Move it aside before fetching again."
        }
        Write-Output "Verified $($brainFile.name)"
        continue
    }
    $brainPending = $brainTarget + '.download'
    $brainUrl = "https://raw.githubusercontent.com/philshiu/Drosophila_brain_model/$($brainManifest.revision)/$($brainFile.name)"
    Write-Output "Downloading $($brainFile.name)..."
    try {
        Invoke-WebRequest -Uri $brainUrl -OutFile $brainPending
        if ((Get-BrainChecksum $brainPending) -ne $brainFile.sha256) {
            throw "Checksum mismatch for $($brainFile.name)."
        }
        Move-Item -LiteralPath $brainPending -Destination $brainTarget
    } finally {
        if (Test-Path -LiteralPath $brainPending) { Remove-Item -LiteralPath $brainPending }
    }
}
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'licenses/Drosophila-model-MIT.txt') -Destination (Join-Path $Destination 'LICENSE') -Force
Write-Output 'Brain data ready. The Rust controller builds its local cache on first use.'
