# Code signing with SignPath Foundation

This is the maintainer checklist for getting Remjet's Windows binaries signed for free by the
[SignPath Foundation](https://signpath.org), so Windows SmartScreen stops warning about an unknown
publisher. Their conditions are at <https://signpath.org/terms>.

## Where things stand

| Requirement (SignPath Foundation terms) | Status |
|---|---|
| OSI-approved license, no proprietary components, no commercial dual licensing | ✅ GPL-3.0-or-later; all dependencies are OSS (see THIRD_PARTY.md) |
| Already released in the form that will be signed | ✅ Zip of `remjet.exe` + `remjet-cli.exe` on GitHub Releases |
| Functionality described on the download page | ✅ README and release notes |
| Built from source in a verifiable way on a trusted build system | ✅ `.github/workflows/release.yml` on GitHub-hosted runners |
| Product name and version in the binaries | ✅ `ProductName = Remjet`, version from `Cargo.toml` (embedded by `crates/remjet/build.rs`) |
| "Code signing policy" section with roles and a privacy statement | ✅ README, *Code signing policy* |
| Multi-factor authentication for every team member (GitHub and SignPath) | ☐ **Maintainer to confirm** (GitHub → Settings → Password and authentication) |
| Every release manually approved for signing | ☐ Set up in SignPath (approver = maintainer) |
| Attribution text on the download page | ☐ After approval (see below) |

## Steps for the maintainer

1. **Enable 2FA on GitHub** if it isn't already on (an authenticator app or passkey).
2. **Apply** at <https://signpath.org/apply>. Typical answers:
   - Project: Remjet, <https://github.com/Tyrannik/remjet>, license GPL-3.0-or-later.
   - Description: free, open-source Windows app that develops Leica M9 raw (DNG) photos with film
     emulation (Kodachrome 64). No network access, no installer.
   - Build system: GitHub Actions, GitHub-hosted runners, workflow `.github/workflows/release.yml`.
   - Artifacts: a zip containing two PE executables (`remjet.exe`, `remjet-cli.exe`) plus
     text files.
   - Releases: <https://github.com/Tyrannik/remjet/releases>.
3. **When accepted**, SignPath sets up an organization and project. In SignPath:
   - Add the **GitHub.com** trusted build system and link it to the project.
   - Use (or paste) the artifact configuration below.
   - Create a signing policy (for example `release-signing`) with yourself as **approver**.
   - Create an API token for CI.
4. **In GitHub** (repo → Settings → Secrets and variables → Actions):
   - Secret `SIGNPATH_API_TOKEN` = the API token.
   - Variable `SIGNPATH_ORGANIZATION_ID` = your SignPath organization ID.
5. **Enable signing in the workflow:** uncomment the *Sign with SignPath* step in
   `.github/workflows/release.yml` (check that `project-slug` and `signing-policy-slug` match what
   SignPath created), remove the "Not yet code-signed" line from the release notes step, and in the
   README replace the "is applying" sentence with the attribution:

   > Free code signing provided by [SignPath.io](https://about.signpath.io), certificate by [SignPath Foundation](https://signpath.org)

6. **Release as usual:** bump the version in `Cargo.toml`, add a `## <version>` section to
   `CHANGELOG.md`, commit, then tag and push (`git tag -a v0.1.0-alpha.3 -m ...` and
   `git push origin v0.1.0-alpha.3`). The workflow builds and tests, uploads the unsigned files to
   SignPath, waits for your approval (you'll get an email or see it in SignPath), and then publishes
   the signed zip.

## Artifact configuration (for SignPath)

The workflow uploads the package folder as a GitHub artifact (SignPath receives it as a zip):

```xml
<?xml version="1.0" encoding="utf-8"?>
<artifact-configuration xmlns="http://signpath.io/artifact-configuration/v1">
  <zip-file>
    <directory path="Remjet-*-windows-x64">
      <pe-file path="remjet.exe" product-name="Remjet" product-version="${version}">
        <authenticode-sign />
      </pe-file>
      <pe-file path="remjet-cli.exe">
        <authenticode-sign />
      </pe-file>
    </directory>
  </zip-file>
</artifact-configuration>
```

Check this against SignPath's current reference (<https://docs.signpath.io/artifact-configuration/reference>)
when setting up the project. SignPath's onboarding can adjust the path patterns and metadata
restrictions (`product-name` / `product-version`) to match the uploaded artifact exactly.
