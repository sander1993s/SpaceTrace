# Code signing policy

## Status: proposed; onboarding pending

This repository contains a proposed integration for the free SignPath Foundation
open-source program. SpaceTrace has not been accepted into the program, no
certificate is configured here, and this document does not claim that existing
downloads are signed. Admission is at the Foundation's discretion. Existing local
builds and ordinary CI remain unsigned.

If approved, the intended attribution is: Free code signing provided by
[SignPath.io](https://about.signpath.io), certificate by
[SignPath Foundation](https://signpath.org). The certificate's publisher is
SignPath Foundation; product metadata remains SpaceTrace / Smet Software Solutions.
Update this status only after approval and a successfully verified signed build.

## Proposed responsibilities and approval

- Author / committer: [sander1993s](https://github.com/sander1993s).
- Reviewer: [sander1993s](https://github.com/sander1993s), including changes from
  contributors and changes to build or signing configuration.
- Signing approver: [sander1993s](https://github.com/sander1993s).

Every signing request must receive manual approval in SignPath. All members with
these roles must enable MFA for GitHub and SignPath before signing is enabled.
These are requirements of the proposed policy, not a statement that those account
settings have already been configured. The API identity may submit requests;
it must not bypass approval. No pull-request build may request release signing.

## Privacy and network behavior

SpaceTrace stores scan history and paths locally in SQLite. It does not send
scan reports or file contents to Smet Software Solutions, provide telemetry,
or require a cloud account. JSON exports are written to a user-selected path.

Network-share scans access the UNC path or mapped drive chosen by the user using
their Windows permissions. For SSH scans, the user supplies the destination,
port, username, key or agent, and starting folder, verifies the host fingerprint,
and starts the scan. OpenSSH sends the bundled metadata helper and request to
that destination; scan metadata returns to the local application. No remote
daemon is installed. These are user-requested transfers, not telemetry.

The installer can download Microsoft's WebView2 bootstrapper if the runtime is
missing. WebView2 is a Microsoft system component and is not signed using this
project's certificate. Its installation and runtime are subject to
[Microsoft's privacy statement](https://privacy.microsoft.com/privacystatement)
and [WebView2 privacy guidance](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/data-privacy).
Do not describe installation as entirely offline.

## Setup after Foundation approval

1. Have SignPath approve this repository and application. Create the project and
   connect GitHub.com as its trusted build system, with origin verification and
   GitHub-hosted runners required. Install the SignPath GitHub App with access
   limited to the approved repository as needed by onboarding.
2. Create a release policy that requires manual approval for every request and
   restricts this repository, `refs/heads/main`, and the release workflow. Confirm
   MFA and the roles above. Use a submitter API token restricted to this policy.
3. Import [.signpath/application.xml](.signpath/application.xml) and
   [.signpath/installer.xml](.signpath/installer.xml) as two artifact configurations.
   They allow only the named first-party executable or installer, with product
   name `SpaceTrace` and the exact product version supplied by the validated
   build. Do not expand the allowlist to third-party binaries.
4. Create a GitHub environment named `signpath-release`, restrict it to `main`,
   and add an available maintainer approval rule. SignPath approval remains
   mandatory even if GitHub environment approval is unavailable.
5. Set the following GitHub Actions configuration. Keep the token in a secret;
   never paste it into source code, logs, or documentation.

| Setting | Kind | Value |
| --- | --- | --- |
| `SIGNPATH_API_TOKEN` | Environment secret | Restricted SignPath submitter token |
| `SIGNPATH_ORGANIZATION_ID` | Environment variable | Approved organization's ID |
| `SIGNPATH_PROJECT_SLUG` | Environment variable | Approved SpaceTrace project slug |
| `SIGNPATH_SIGNING_POLICY_SLUG` | Environment variable | Policy requiring manual approval |
| `SIGNPATH_APPLICATION_CONFIG_SLUG` | Environment variable | Imported application XML configuration slug |
| `SIGNPATH_INSTALLER_CONFIG_SLUG` | Environment variable | Imported installer XML configuration slug |
| `SIGNPATH_EXPECTED_PUBLISHER_SUBJECT` | Environment variable | Exact approved certificate Subject distinguished name, verified through SignPath |
| `SIGNPATH_ENABLED` | Repository variable | `true`, only after completing setup |

Do not guess the certificate subject or copy it from an unverified download.
The enabled flag must be a repository variable because the job condition is
evaluated before its environment is loaded. The organization and slugs are
identifiers, not secrets. No credentials or enrollment IDs are provided here.

## Manually signing a release build

Run **Sign Windows release (pending SignPath onboarding)** from GitHub Actions
on `main`. It signs the selected commit; update the three release manifests to
the same stable `major.minor.patch` version before dispatching. The workflow is
disabled until `SIGNPATH_ENABLED=true` and fails if required configuration is
missing. It never creates or publishes a GitHub release or changes website files.

The workflow runs the existing Windows build and tests. The first installer that
build creates is discarded as a release candidate. It then uploads the application
for origin verification, waits up to an hour for signing approval, verifies its
signature, replaces the local application with that signed file, and calls Tauri's
`bundle --bundles nsis` without compiling again. A hash check ensures bundling did
not change the signed application. The resulting installer is uploaded for a
second signing request and manual approval.

Product name and version are checked before submission and after signing.
Windows must report a valid embedded Authenticode signature, the configured
publisher subject, and a timestamp certificate for both final executables.
Failure, rejection, missing timestamps, or timeout stops the workflow; it has
no unsigned fallback. The final download artifact contains the signed application,
signed installer, and SHA-256 hashes. Download and inspect that artifact before
separately publishing it. Intermediate artifacts named `UNSIGNED-DO-NOT-DISTRIBUTE`
are necessary for SignPath origin verification, expire after one day, and must
never be used as downloads on the product website.

The workflow signs the application and the **outer NSIS installer only**.
The NSIS-generated uninstaller is not signed by this two-stage integration.
Signing every installer-generated executable requires a further, SignPath-approved
Tauri signing integration. Do not advertise complete signing coverage. During
onboarding, verify that SignPath accepts this scope before enabling the workflow.
Signing also does not guarantee immediate SmartScreen reputation.

## Verification boundaries

The workflow and artifact definitions can be checked statically before enrollment.
A real end-to-end signing run requires Foundation approval, account configuration,
the restricted token, and the two manual approvals. The first run must confirm
Tauri's actual PE metadata, the configured certificate, the installed application's
signature, and normal install/uninstall behavior before public distribution.
No successful production signing run is implied by these files.

References: [Foundation terms](https://signpath.org/terms),
[SignPath GitHub integration](https://docs.signpath.io/trusted-build-systems/github),
[artifact configurations](https://docs.signpath.io/artifact-configuration/),
[Tauri's separate build and bundle commands](https://v2.tauri.app/distribute/).
