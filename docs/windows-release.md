# Windows Release

`tally-windows-x86_64.exe` is the single graphical installer for both Codex and
Claude Code. GitHub Actions runs installation, handshake, hook, forwarding,
retry, custom-path, and uninstall tests against the exact executable before it
is published.

The executable is not yet Authenticode signed. Microsoft Defender SmartScreen
may show an unrecognized-app warning, and organization policy may block it. This
cannot be fixed by renaming the file or moving the installed hook handler; a
trusted Windows publisher signature is the proper future solution.

To verify the release checksum in PowerShell after downloading `SHA256SUMS`:

```powershell
$name = "tally-windows-x86_64.exe"
$expected = ((Select-String -Path .\SHA256SUMS -Pattern "  $name$").Line -split "\s+")[0]
$actual = (Get-FileHash ".\$name" -Algorithm SHA256).Hash.ToLower()
if ($actual -ne $expected) { throw "Checksum mismatch" }
```

A checksum detects corruption or a mismatched download. It does not establish a
trusted Windows publisher identity.
