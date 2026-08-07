# Windows release signing

Tally's public Windows executables must be Authenticode-signed before release. The release workflow uses Microsoft Azure Artifact Signing with a Public Trust certificate profile and refuses to publish unsigned Windows assets.

## Azure setup

1. Create an Azure Artifact Signing account.
2. Complete public identity validation for OpenOrigins.
3. Create a `PublicTrust` certificate profile.
4. Create a Microsoft Entra application for GitHub Actions.
5. Add a federated credential restricted to the `OpenOrigins/tally` repository and the release environment or tag policy.
6. Give that identity the `Artifact Signing Certificate Profile Signer` role on the certificate profile.

## GitHub configuration

Add these Actions secrets:

- `AZURE_CLIENT_ID`: Entra application client ID.
- `AZURE_TENANT_ID`: Entra directory tenant ID.
- `AZURE_SUBSCRIPTION_ID`: Azure subscription ID containing Artifact Signing.

Add these Actions variables:

- `AZURE_ARTIFACT_SIGNING_ENDPOINT`: Regional endpoint such as `https://weu.codesigning.azure.net/`.
- `AZURE_ARTIFACT_SIGNING_ACCOUNT_NAME`: Artifact Signing account name.
- `AZURE_ARTIFACT_SIGNING_CERTIFICATE_PROFILE_NAME`: Public Trust certificate profile name.

No certificate private key or Azure client secret is stored in GitHub. GitHub authenticates to Azure through OpenID Connect.

## Release verification

The Windows release job signs both executables with SHA-256 and an RFC 3161 timestamp before running the end-user installation test. It then requires `Get-AuthenticodeSignature` to report `Valid` and a timestamp certificate for both files. Only those tested, signed bytes are packaged and published.

Signing establishes a stable publisher identity but does not guarantee immediate SmartScreen reputation for a new publisher. Sign every release with the same profile. If Microsoft Defender Antivirus reports a specific detection, submit the signed release file through the [Microsoft Security Intelligence file-submission portal](https://www.microsoft.com/en-us/wdsi/filesubmission) as a software developer and retain the submission result with the release record.
