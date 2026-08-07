# Windows releases

Tally's Windows executables are currently published without an Authenticode signature. Microsoft Defender SmartScreen may therefore show an unrecognized-app warning, and managed enterprise devices may block execution under organization policy.

Every executable is built and exercised by the Windows end-user installation test before release. Each release also includes a `.sha256` file generated from the tested executable. Verify a download in PowerShell with:

```powershell
$expected = (Get-Content .\tally-codex-windows-x86_64.exe.sha256).Split()[0]
$actual = (Get-FileHash .\tally-codex-windows-x86_64.exe -Algorithm SHA256).Hash.ToLower()
if ($actual -ne $expected) { throw "Checksum mismatch" }
```

Checksums detect accidental corruption and mismatches between the downloaded files, but they do not establish a trusted Windows publisher identity. A future signing implementation must sign before the end-user tests and publish the exact tested bytes.
