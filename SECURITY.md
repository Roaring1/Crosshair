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

Crosshair is a single-user desktop overlay for Linux and Windows, not a
network service. Its control interfaces are local to the user's desktop
session; neither backend opens a listening TCP port.

- Linux uses a Unix-domain socket under the user's runtime directory. The
  application creates a private runtime subdirectory and owner-restricted
  socket. IPC clients must have verifiable peer credentials and run under the
  same UID as the daemon. The server fails closed if credentials cannot be
  verified.
- Windows uses a named pipe whose name is derived from the current domain,
  username and profile. The server uses the process token's default DACL,
  creates a non-inheritable pipe handle, requires the first pipe instance, and
  sets `PIPE_REJECT_REMOTE_CLIENTS`. This restricts access to local clients
  permitted by the process token's DACL and rejects remote pipe clients.
- Configuration files are capped at 1 MiB. Both platform readers reject
  symlink and non-regular config paths before reading. JSON parsing rejects
  duplicate keys, non-standard numeric constants and excessive nesting; the
  encoder refuses NaN and Infinity. Writes use an exclusive temporary file and
  atomic replacement. Linux additionally pins parent directories through
  no-follow file descriptors, refuses multiply-linked or non-owned targets,
  and applies owner-private permissions. Windows opens the temporary file
  exclusively and replaces the destination with `MoveFileExW` write-through
  semantics.
- On Linux, directory, lock-file and log-file opens use verified file
  descriptors, no-follow and nonblocking flags, and private modes. The socket
  is created with owner-only permissions at bind time. Startup refuses to
  delete a non-socket entry at the socket path, and shutdown unlinks only the
  exact socket inode created by that daemon instance. Restart setup closes
  acquired descriptors if pipe setup or helper spawning fails.
- On Linux, external desktop/system helper programs are resolved only from
  root-owned system directories and rejected if the current user can write
  the executable or any parent directory. The check accounts for groups and
  ACLs; PATH entries are not trusted for these helper invocations.
- The Linux installer writes only to user-level application paths and does
  not require root privileges. Legacy systemd files are not removed unless
  the trusted system manager confirms disable and daemon-reload success.
- The Windows installer writes under the current user's `%LOCALAPPDATA%` and
  `%APPDATA%`, updates only the current user's PATH and startup registry key,
  and does not require administrator rights.

This is not a security boundary between processes running as the same user.
Malware or another untrusted process already running with the user's UID can
generally access that user's files and invoke user-session IPC. Crosshair
does not claim to sandbox or contain same-user malware.

## Validation

The project runs Ruff, Python byte-compilation, pytest, Bandit medium/high
severity checks in the Python 3.10–3.14 matrix. Dependency advisory scanning
runs in a separate Python 3.14 job using `requirements-audit.txt`, which pins
the fixed `filelock==4.2.10` release (which requires Python 3.11+;
older versions are affected by current advisories). GitHub Actions references
and direct development/audit-tool versions are pinned; Dependabot checks for
updates weekly.
