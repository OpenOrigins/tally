# Windows release signing

Tally's public Windows executables must be Authenticode-signed before release. The release workflow uses SignPath and refuses to publish unsigned Windows assets. This does not require an Azure or AWS account, and the signing private key never enters GitHub Actions.

## SignPath setup

1. Choose and add an OSI-approved license to the repository. SignPath Foundation requires one for its free open-source program; Tally does not currently declare a license.
2. Publish the code-signing policy required by SignPath Foundation, including project roles and privacy behavior.
3. [Apply to SignPath Foundation](https://signpath.org/apply.html) for free open-source signing.
4. Install the SignPath GitHub App for `OpenOrigins/tally` and configure GitHub.com as the trusted build system.
5. Create a Tally project, release signing policy, and artifact configuration in SignPath.
6. Configure the artifact as a ZIP containing `tally-codex.exe` and `tally-claude.exe`, with Authenticode signing enabled for both PE files.
7. Create an API token for a SignPath user with submitter permission for that signing policy.

## GitHub configuration

Add these Actions secrets:

- `SIGNPATH_API_TOKEN`: SignPath API token with submitter permission.

Add these Actions variables:

- `SIGNPATH_ORGANIZATION_ID`: SignPath organization ID.
- `SIGNPATH_PROJECT_SLUG`: Tally project slug.
- `SIGNPATH_SIGNING_POLICY_SLUG`: Release signing policy slug.
- `SIGNPATH_ARTIFACT_CONFIGURATION_SLUG`: Artifact configuration slug.

The SignPath artifact configuration can use this structure:

```xml
<artifact-configuration xmlns="http://signpath.io/artifact-configuration/v1">
  <zip-file>
    <pe-file path="tally-codex.exe">
      <authenticode-sign description="Tally Codex" description-url="https://github.com/OpenOrigins/tally" />
    </pe-file>
    <pe-file path="tally-claude.exe">
      <authenticode-sign description="Tally Claude Code" description-url="https://github.com/OpenOrigins/tally" />
    </pe-file>
  </zip-file>
</artifact-configuration>
```

## Release verification

The Windows release job uploads both executables as a temporary GitHub Actions artifact, asks SignPath to sign them, deletes the unsigned artifact, and requires `Get-AuthenticodeSignature` to report `Valid` with a timestamp certificate. The complete end-user installation test then runs against those exact signed bytes. Only tested, signed files are packaged and published.

Signing establishes a stable publisher identity but does not guarantee immediate SmartScreen reputation for a new publisher. Sign every release with the same profile. If Microsoft Defender Antivirus reports a specific detection, submit the signed release file through the [Microsoft Security Intelligence file-submission portal](https://www.microsoft.com/en-us/wdsi/filesubmission) as a software developer and retain the submission result with the release record.
