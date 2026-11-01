# Security Policy

## Supported version

Security fixes are maintained on the `main` branch. Please update to the
latest published source before reporting an issue.

## Reporting a vulnerability

Please do not publish suspected vulnerabilities in a public issue or pull
request. Use GitHub's private vulnerability reporting / Security Advisory
feature for this repository. If private reporting is unavailable, contact
the maintainer privately through the GitHub profile and include a concise
reproduction, affected version/commit, and impact.

Do not include credentials, private configuration files, or unrelated
personal data in a report.

## Security model

Crosshair is a single-user Linux desktop overlay, not a network service.

- The control interface is a Unix-domain socket under the user's runtime
  directory. The app creates a private runtime subdirectory and a
  permission-restricted socket.
- On Linux, IPC clients must have verifiable peer credentials and must run
  under the same UID as the daemon. The server fails closed if credentials
  cannot be verified.
- Config reads refuse symlinks and non-regular files and are capped at
  1 MiB. Oversized config files are left untouched while in-memory defaults
  are used. Duplicate JSON keys, non-standard numeric constants and excessive
  nesting are rejected safely. Config writes use unpredictable exclusive
  temporary files and rename relative to a pinned, no-follow parent directory
  descriptor.
- IPC JSON parsing rejects duplicate keys and non-standard numeric constants;
  the encoder refuses NaN and Infinity values.
- Directory modes, lock-file modes, normal log-file modes, and delayed
  restart-helper log modes are applied through verified file descriptors.
  Lock and log opens are no-follow and nonblocking; they reject non-regular
  files. Restart setup closes every acquired descriptor if pipe setup or helper
  spawning fails. The socket is created with owner-only
  permissions at bind time. Startup refuses to delete a non-socket entry at
  the socket path, and shutdown unlinks only the exact socket inode created
  by that daemon instance.
- External desktop/system helper programs are resolved only from root-owned
  system directories and rejected if the current user can write the file or
  any parent directory. This check accounts for the user's groups and ACLs;
  PATH entries are not trusted for these helper invocations.
- The installer writes only to user-level application paths; it does not
  require root privileges. Legacy systemd files are not removed unless the
  trusted system manager confirms disable and daemon-reload success.

This is not a security boundary between processes running as the same user.
Malware or another untrusted process already running with the user's UID can
generally access that user's files and invoke user-session IPC. Crosshair
does not claim to sandbox or contain same-user malware.

## Validation

The project runs Ruff, Python byte-compilation, pytest, Bandit medium/high
severity checks in the Python 3.10–3.14 matrix. Dependency advisory scanning
runs in a separate Python 3.14 job using `requirements-audit.txt`, which pins
the fixed `filelock==4.2.1` release (the patched release requires Python 3.11+;
older versions are affected by current advisories). GitHub Actions references
and direct development/audit-tool versions are pinned; Dependabot checks for
updates weekly.
