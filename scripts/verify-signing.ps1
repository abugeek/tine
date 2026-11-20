[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$Path,

    [switch]$AllowUnsigned
)

$ErrorActionPreference = "Stop"

if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
    Write-Error "File not found for signature verification: $Path"
    exit 1
}

$sig = Get-AuthenticodeSignature -FilePath $Path
switch ($sig.Status) {
    "Valid" {
        if ($null -eq $sig.TimeStamperCertificate) {
            Write-Error "Signature on '$Path' is missing an RFC 3161 / Authenticode timestamp certificate."
            exit 1
        }
        $subj = $sig.SignerCertificate.Subject
        $thumb = $sig.SignerCertificate.Thumbprint
        Write-Host "OK: '$Path' signed by '$subj' (thumbprint=$thumb), timestamped by '$($sig.TimeStamperCertificate.Subject)'."
        exit 0
    }
    "NotSigned" {
        if ($AllowUnsigned) {
            Write-Host "DRY-RUN: '$Path' is unsigned (-AllowUnsigned specified; passing check)."
            exit 0
        }
        Write-Error "File '$Path' is NotSigned (set -AllowUnsigned for local dry-runs)."
        exit 1
    }
    default {
        Write-Error "Signature check failed for '$Path': Status=$($sig.Status) Message=$($sig.StatusMessage)"
        exit 1
    }
}
