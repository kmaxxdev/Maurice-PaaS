# ADR-008: Ubuntu 26.04 LTS as the primary host

- **Status:** accepted, supersedes [[003-fedora-family-host-and-selinux]]
- **Date:** 2026-10-22
- **Issue:** #4

## Context

ADR-003 chose the Fedora family because Ubuntu 24.04 ships Podman 4.9, too old for Quadlet. Checking the actual archive showed Ubuntu 26.04 LTS ships Podman 5.7.0 with crun 1.21, netavark 1.16 and aardvark-dns 1.16, newer than Debian 13's 5.4. The production server already runs Ubuntu 24.04 and an in-place upgrade to 26.04 is cheap, while a reinstall onto Fedora is not. Ubuntu LTS also gives five years of support against Fedora's yearly release upgrade.

## Decision

- **Primary host: Ubuntu 26.04 LTS.** Development VM, CI and production all run it. The production server upgrades from 24.04 to 26.04 before the Coolify migration.
- **Secondary targets: Fedora Server and Debian 13.** Kept in CI once CI exists, so the Fedora-first Podman features and the SELinux path stay exercised.
- **Generation stays SELinux-aware.** The Quadlet generator still emits `:Z` / `:z` on volumes; on an AppArmor host these flags are inert, on Fedora they are required. No host-specific branches in the generator.
- **AppArmor is left at the Ubuntu default.** Podman ships its own profile; the panel does not manage AppArmor in Alpha.

## Alternatives considered

- **Fedora Server** (ADR-003). Freshest Podman, but yearly release upgrades on a production host and a reinstall to get there.
- **AlmaLinux 10.** Ten-year support, but Podman lags and the reinstall remains.
- **Debian 13.** Stable, but Podman 5.4 is older than Ubuntu 26.04's and Debian freezes for two years.

## Consequences

- Spike #4 creates an Ubuntu 26.04 VM by default; the script can also create a Fedora VM for compatibility checks.
- Ubuntu's Podman packaging lags upstream by roughly one LTS cycle; features newer than 5.7 are not assumed until the next LTS.
- Documentation and the installer target `apt` first, `dnf` second.
