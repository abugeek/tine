# Security Policy

## Supported versions

Tine is in beta: only the latest release from
[Releases](https://github.com/abugeek/tine/releases) receives fixes.

## Reporting a vulnerability

**Please don't open a public issue.** Use GitHub Security Advisories:

- **Report privately:** Open a draft advisory via the repository's **Security** tab (*Report a vulnerability* / GitHub Security Advisories).
- **Preferred languages:** English.
- **Response target:** within 7 days; fixes or mitigations announced with the
  release that contains them.

## Scope & architecture notes (what reviewers should know)

- **No audio DSP inside Windows' audio service.** v0.1 deliberately ships *no*
  APO and registers nothing in `audiodg.exe` (decision
  [D-018](docs/project/06-decisions.md)): a crash in a third-party APO would
  take down all system audio. Tine runs as an ordinary per-user process and
  routes audio through a virtual cable. A driver-packaged APO (with proper
  signing) is a possible post-v0.1 direction and will get its own threat
  model.
- **No network code.** The binary contains no HTTP client — there is not
  even an update check ([D-031](docs/project/06-decisions.md)). Microphone
  audio stays in process memory; nothing is uploaded. `tine diag` contains
  logs and device/process metadata only (device ids are FNV-1a hashed) and
  is written only when you run it.
- **Elevation:** the main app never elevates — only the MSI installer does
  (one UAC prompt), and only the shell's autostart value
  (`HKCU\…\CurrentVersion\Run`) is written per-user. The engine keeps zero
  HKLM state of its own beyond MSI-installed files and a marker key.
- **Shared memory:** machine-wide state is limited to
  `%ProgramData%\Tine\endpoints\` (per-endpoint DSP boot parameters, JSON).
  The write ACL is Users-modify on that folder only; every parameter is
  validated and range-clamped on load, so a tampered file cannot push the
  DSP out of range.
- **Diagnostics:** disclose away — `tine diag` is the supporting artifact;
  ask for it in reports.

## Signing

The beta ships unsigned (with published SHA-256 sums and documented
SmartScreen steps). Public code-signing via SignPath Foundation (or Azure
Trusted Signing) is a maintainer runbook item; the runbook lives in
[`docs/tasks/done/T23-packaging-release.md`](docs/tasks/done/T23-packaging-release.md).
