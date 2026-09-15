# tamatoa

![Rust CI](https://github.com/joshfrogers/tamatoa/actions/workflows/rust.yml/badge.svg)
[![license: GPL-3.0](https://img.shields.io/badge/license-GPL--3.0-blue.svg)](LICENSE.md)

<img src="./img/DALL-E-SAC.png" width="100" height="100" align="right" alt="tamatoa crab">

Forensic evidence collector for Windows, Linux and macOS. It gathers a
standard set of artifacts (plus whatever paths you give it) into one ZIP
with a SHA-256 manifest. CLI flags are compatible with
[CyLR](https://github.com/orlikoski/CyLR/), so existing runbooks keep
working.

Built to be run unattended by an agent (Velociraptor, Ansible, Intune,
psexec, ssh) during an incident.

## Usage

```sh
tamatoa                        # default artifacts, name = <host>-<UTC ts>.zip
tamatoa -q --json -od /var/tmp # what an orchestrator would run
```

With `--json`, stdout is a single JSON document and logs go to stderr.
Exit codes:

| Code | Meaning |
|-----:|---------|
| 0 | everything requested was collected |
| 1 | partial; look for `{"failed": reason}` entries in the manifest |
| 2 | fatal (bad usage, unwritable output); no archive produced |

Missing optional artifacts (a user without `.bash_history`, etc.) count
as `missing`, not `failed`, and do not change the exit code.

`tamatoa --help` lists all flags. Notes on a few:

- `-c FILE` collects from a config file instead of the default set;
  `-d FILE` adds a config file on top of the defaults. Both may be given.
  Config format: one path or glob per line, `#` comments, `$VAR` /
  `${VAR}` / `%VAR%` expansion. See CUSTOM_PATH_TEMPLATE.txt.
- Positional `PATHS...` (files, directories, or globs) are collected in
  addition to whatever else is selected.
- `-of` must be a bare file name; combine with `-od DIR`. Existing files
  are never overwritten without `--force`.
- `-hf` is accepted for CyLR compatibility and does nothing; every
  artifact is hashed anyway.
- `--max-file-bytes` / `--max-total-bytes` (defaults 128 GiB / 1 TiB)
  bound the collection against infinite or absurd sources.

Run privileged where you can. Unprivileged runs still work; artifacts
you cannot read are recorded as failures.

## What it collects
Windows: `$Recycle.Bin` metadata, Event Logs, Prefetch, SRU, scheduled
tasks, startup items, Amcache, SetupAPI log, hosts file, minidumps,
NetSetup.LOG, Windows Defender MPLogs, user hives (NTUSER.DAT /
UsrClass.dat + transaction logs, plus the SAM/SOFTWARE/SYSTEM/DEFAULT/
SECURITY system hives; locked files are read through raw volume access),
Recent/Jump Lists/WebCache, Chrome and Edge history, cookies, logins and
extensions for every browser profile (incl. Local State key material),
Firefox profiles, PowerShell console history, `$MFT`/`$LogFile`, and
`$UsnJrnl:$J` with `--usnjrnl`.

Linux: shell histories and dotfiles for every account with a real home
(from /etc/passwd, not just /home/*), ssh material, cron/at, systemd
units including per-user units and autostart entries, /var/log
recursively, sudoers and sudoers.d, ld.so.preload/conf.d (rootkit
check), profile/profile.d, issue/motd/rc.local, fstab, resolv.conf,
apt/yum sources, cloud-init config and per-instance data (user-data
often holds injected credentials), dpkg installed-package state,
PAM and sshd config, grub and ACPI tables.

macOS: per-account shell histories and ssh dotfiles, Safari history and
cookies, Chrome (all profiles) and Firefox data, TCC databases (user
and system), .fseventsd, unified log plus its uuidtext string store,
launch agents (system, global, per-user) and daemons, startup items,
/var/log and diagnostics, hosts/passwd/group, sudoers and sudoers.d,
master.passwd, pf.conf, ssh config, at jobs.

The manifest records exactly what any given run attempted and what
happened to each entry.

## Behavior worth knowing

- Symlinks and special files (fifos, devices, sockets) are never
  followed or opened. Directories are never read as files.
- Failed artifacts are recorded with the real OS error. Nothing is
  silently skipped and no empty placeholder entries are written.
- A panic while processing one artifact (including inside the NTFS
  parser) is caught, recorded as a failure, and the archive still
  completes.
- Archives are written 0600, entry names are host-relative and cannot
  contain `..` or drive letters. Sorted entries, fixed timestamps: two
  identical runs produce identical bytes.
- File names and paths are escaped in all log output (`"\e[31m..."`
  style) so a compromised host's file names cannot spoof the console.

## Output

Given `-od /out -of host.zip`:

- `/out/host.zip` - the evidence archive, containing
  `tamatoa_manifest.json` (per-source status and SHA-256).
- `/out/host.zip.manifest.json` - sidecar with the same manifest plus
  the archive's own SHA-256, tool version and git hash, and timing.

## Building

```sh
cargo build --release          # native
cargo build --release --target x86_64-unknown-linux-musl  # static binary
```

No C toolchain is needed; the dependency tree is pure Rust.

Releases (tags `vX.Y.Z`, must match Cargo.toml) ship
`tamatoa-x86_64-unknown-linux-musl.tar.gz` (static; runs on any glibc or
musl host), `tamatoa-x86_64-pc-windows-msvc.zip`, and
`tamatoa-aarch64-apple-darwin.tar.gz`, plus `SHA256SUMS`. Each binary is
smoke-tested on its platform before packaging.

CI runs on every PR: fmt, clippy `-D warnings`, the test suite on
Linux/Windows/macOS runners, release-profile tests, `cargo check` for
all three release targets, and `cargo audit` (plus weekly). Actions are
pinned to commit SHAs.

## License

GPL-3.0, see LICENSE.md.

Image attribution: DALL-E / OpenAI.
