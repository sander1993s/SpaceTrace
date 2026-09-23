#requires -Version 7.0
[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string]$Path,
    [Parameter(Mandatory)] [ValidatePattern('^\d+\.\d+\.\d+$')] [string]$Version,
    [switch]$RequireSignature,
    [string]$ExpectedPublisherSubject
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$file = Get-Item -LiteralPath $Path
if ($file.PSIsContainer -or $file.Extension -ine '.exe') {
    throw "Expected one executable: $Path"
}
$metadata = $file.VersionInfo
if ($metadata.ProductName -cne 'SpaceTrace') {
    throw "Unexpected product name in $($file.Name): '$($metadata.ProductName)'"
}
# NSIS may express the same numeric product version with a fourth zero component.
if ($metadata.ProductVersion -cnotin @($Version, "$Version.0")) {
    throw "Unexpected product version in $($file.Name): '$($metadata.ProductVersion)'"
}

$publisher = $null
if ($RequireSignature) {
    if ([string]::IsNullOrWhiteSpace($ExpectedPublisherSubject)) {
        throw 'An exact expected certificate subject is required for signature verification.'
    }
    $signature = Get-AuthenticodeSignature -LiteralPath $file.FullName
    if ($signature.Status -ne 'Valid' -or $signature.SignatureType -ne 'Authenticode') {
        throw "Invalid Authenticode signature on $($file.Name): $($signature.Status)"
    }
    if ($null -eq $signature.SignerCertificate -or
        $signature.SignerCertificate.Subject -cne $ExpectedPublisherSubject) {
        throw "Unexpected signing certificate on $($file.Name)."
    }
    if ($null -eq $signature.TimeStamperCertificate) {
        throw "Missing trusted timestamp on $($file.Name)."
    }
    $publisher = $signature.SignerCertificate.Subject
}

[pscustomobject]@{
    Name = $file.Name
    ProductVersion = $metadata.ProductVersion
    Publisher = $publisher
    SHA256 = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash
}
