$ErrorActionPreference = "Stop"

function Invoke-Cargo {
    param([string[]]$CargoArgs)

    & cargo @CargoArgs
    if ($LASTEXITCODE -ne 0) {
        throw "cargo $($CargoArgs -join ' ') failed with code $LASTEXITCODE"
    }
}

Invoke-Cargo -CargoArgs @("fmt", "--all", "--check")
Invoke-Cargo -CargoArgs @("clippy", "--locked", "--workspace", "--all-targets", "--all-features", "--", "-D", "warnings")
Invoke-Cargo -CargoArgs @("test", "--locked", "--workspace", "--all-features")
Invoke-Cargo -CargoArgs @("test", "--locked", "--workspace", "--doc")
Invoke-Cargo -CargoArgs @("doc", "--locked", "--workspace", "--no-deps")
Invoke-Cargo -CargoArgs @("build", "--locked", "--workspace", "--release")
