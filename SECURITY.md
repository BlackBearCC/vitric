# Security Policy

## Reporting a vulnerability

**Please do not report security vulnerabilities through public GitHub issues.**

Report them through GitHub's private vulnerability reporting:

<https://github.com/BlackBearCC/vitric/security/advisories/new>

Include as much of the following as you can:

- The affected version or commit, and your operating system.
- Steps to reproduce, ideally with a minimal game project (a `vitric.json`
  project like the ones under `examples/`).
- The impact you see: crash, sandbox escape, deterministic-replay violation,
  injection, etc.

You will receive an acknowledgement as soon as possible. We will investigate,
keep you informed of progress, and credit you in the advisory when the fix
ships (unless you prefer to remain anonymous). Please give us a reasonable
window to fix the issue before any public disclosure.

Areas of particular security interest for this project:

- **The script sandbox** (`vitric-script`) — game scripts may come from
  untrusted sources (LLM-generated or user-authored); escapes of the QuickJS
  sandbox or bypasses of the declared reads/writes enforcement are high-impact.
- **The control plane** (`vitric-control`) — the HTTP JSON-RPC port grants full
  control over a running game; issues around unintended exposure beyond
  localhost matter.
- **Asset and data loading** — malformed PNGs, scenes, schemas, or archives
  (`vitric assets`, `vitric bundle`) that cause panics or memory unsafety.
- **Determinism violations** — inputs that make replay diverge undermine the
  engine's verification guarantees.

## Supported versions

Vitric is pre-1.0. Only the latest release receives security fixes; there are
no backports to older versions.

| Version | Supported          |
| ------- | ------------------ |
| 0.1.x (latest) | :white_check_mark: |
| older   | :x:                |

## Scope notes

- Games you build with Vitric execute JavaScript/TypeScript systems inside the
  engine's sandbox. Vulnerabilities in *your game logic* are out of scope;
  vulnerabilities in the *sandbox itself* are in scope.
- The control plane listens on `127.0.0.1` only. If you proxy or tunnel it to
  a public interface, securing that path is your responsibility.
