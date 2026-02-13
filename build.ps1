param(
    [Parameter(Mandatory=$true)]
    [string]$BinName,
    
    [switch]$Release
)

$ErrorActionPreference = "Stop"

# Build the binary
if ($Release) {
    cargo build --release --bin $BinName
    $SourceDir = "build\release"
} else {
    cargo build --bin $BinName
    $SourceDir = "build\debug"
}

if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}

# Create output directory
$OutputDir = "build\$BinName"
if (-not (Test-Path $OutputDir)) {
    New-Item -ItemType Directory -Path $OutputDir -Force | Out-Null
}

# Copy the binary
$BinaryName = "$BinName.exe"
Copy-Item "$SourceDir\$BinaryName" "$OutputDir\$BinaryName" -Force

Write-Host "Built: $OutputDir\$BinaryName" -ForegroundColor Green
