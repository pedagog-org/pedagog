# code-server Reference

> Keep this document up-to-date as we learn more about code-server configuration.
> It is the canonical reference for how Pedagog configures and restricts code-server.

---

## Overview

[code-server](https://github.com/coder/code-server) runs VS Code in a browser. Pedagog
uses it for interactive exam sessions. It inherits VS Code's enterprise policy system,
which is the primary mechanism for machine-level lockdown.

---

## Configuration Sources (priority order, highest first)

1. CLI flags
2. Config file (`~/.config/code-server/config.yaml`)
3. Built-in defaults

**Config file format:**
```yaml
bind-addr: 127.0.0.1:8080
auth: none
```

Each CLI flag maps to a config key. We use CLI flags at container start (via
`entrypoint.sh`) rather than the config file, so configuration is visible in the process
invocation and not spread across two places.

---

## How Pedagog Starts code-server

```sh
code-server \
  --bind-addr 127.0.0.1:8080 \
  --auth none \
  --disable-file-downloads \
  --disable-file-uploads \
  --disable-update-check \
  --disable-workspace-trust \
  /home/student/workspace
```

**`--auth none`** — authentication is handled upstream by Traefik + platform middleware.
code-server itself does not require a password.

**`--disable-file-downloads` / `--disable-file-uploads`** — removes the download option
from the file context menu and disables drag-and-drop file uploads. Prevents students from
exfiltrating or injecting files outside the submission flow.

**`--disable-update-check`** — suppresses the 6-hourly update check. Images are
versioned; in-container updates are not permitted.

**`--disable-workspace-trust`** — bypasses the workspace trust prompt that appears when
opening an untrusted folder. Removes friction without a meaningful security tradeoff in a
container environment.

**`/home/student/workspace`** — sets the default open folder.

### Flags we considered but do not use

| Flag | Reason not used |
| --- | --- |
| `--idle-timeout-seconds` | Session lifecycle is managed by the Pedagog daemon and platform watchdog, not code-server |
| `--disable-telemetry` | Covered by `TelemetryLevel` policy; policy is the single source of truth |
| `--auth password` / `--hashed-password` | Auth handled by platform middleware |

---

## Policy System

**VS Code enterprise policies work in our patched code-server build.** The stock
code-server does not support policies ([coder/code-server#7672](https://github.com/coder/code-server/issues/7672)),
but Pedagog ships a `server-policy.diff` patch that wires policy reading into the
server startup path. Policy is read from `/etc/vscode/policy.json` at server start.

Policy is the **primary** enforcement mechanism. It runs server-side and cannot be
bypassed from the browser.

### policy.json format

```json
{
  "AllowedExtensions": {
    "publisher.extension-id": true
  },
  "DisableAIFeatures": true,
  "TelemetryLevel": "off",
  "UpdateMode": "none"
}
```

**`AllowedExtensions` must be an object** (`{ "id": true }`), not an array. An array
is silently ignored by `getAllowedExtensionsValue()` — extensions would be unrestricted.

**`TelemetryLevel`** is the correct key (not `EnableTelemetry`, which does not exist
as a policy key in VS Code).

### Current policy.json (code-server-test image)

```json
{
  "AllowedExtensions": {
    "llvm-vs-code-extensions.vscode-clangd": true
  },
  "DisableAIFeatures": true,
  "ChatPluginsEnabled": false,
  "Claude3PIntegration": false,
  "Codex3PIntegration": false,
  "ChatAgentMode": false,
  "TelemetryLevel": "off",
  "UpdateMode": "none",
  "EnableFeedback": false
}
```

**`llvm-vs-code-extensions.vscode-clangd`** — use clangd, not `ms-vscode.cpptools`.
Microsoft does not publish cpptools to Open-VSX, which is the default marketplace for
code-server.

### AllowedExtensions behavior

Non-allowed extensions are **silently omitted** from marketplace search results — students
see no error and receive no indication that filtering is happening. Installed extensions
not in the allowlist are shown as disabled with no install/enable option.

### DisableAIFeatures and the command palette

When `DisableAIFeatures` is active, VS Code sets the `chatSetupHidden` context key,
which hides Copilot UI. Our `hide-ai-command.diff` patch additionally gates the
"Use AI Features with Copilot" command palette entry on `!config.chat.disableAIFeatures`,
so it does not appear when the policy is active.

## Machine Settings

Machine settings live at `~/.local/share/code-server/Machine/settings.json`. Written
into the image at build time.

**Role:** UX defaults and convenience. Policy is the enforcement layer — machine settings
are belt-and-suspenders for settings that policy does not cover.

Settings scopes that machine settings *cannot* lock (students can override in
`~/.local/share/code-server/User/settings.json`):
- APPLICATION scope (e.g., `telemetry.telemetryLevel`, `update.mode`)
- RESOURCE/WINDOW scope (e.g., `chat.disableAIFeatures`)

These are overridable by students, but useless without network access to the relevant
endpoints (telemetry, Copilot, update servers).

### Machine settings file used by Pedagog

Policy covers `DisableAIFeatures`, `TelemetryLevel`, and `UpdateMode` — those are not
duplicated here. Machine settings are only for settings that have no policy equivalent.

```json
{
  "extensions.autoUpdate": false,
  "extensions.autoCheckUpdates": false,
  "workbench.startupEditor": "none",
  "workbench.tips.enabled": false,
  "workbench.welcomePage.walkthroughs.openOnInstall": false
}
```

### What students can still change

Students can set anything in `~/.local/share/code-server/User/settings.json`:
- Editor preferences, themes, font size, keybindings
- APPLICATION/RESOURCE-scoped settings (but policy and network rules make most
  of these overrides pointless)

## Extension Gallery

The extension marketplace URL is not patched in the image. The marketplace UI is
visible but all network calls to Open-VSX are blocked by k8s NetworkPolicy. Students
will see the marketplace panel but searches and installs will fail silently.

A student could technically set `extensions.gallery.serviceUrl` in their
`User/settings.json` to another URL — network policy blocking at egress makes this
irrelevant.

## Network Policy (Enforcement Layer)

k8s NetworkPolicy egress rules are the actual enforcement mechanism for extensions,
AI features, and telemetry. Settings and machine settings files provide UX defaults
only. Network policy makes bypassing those defaults pointless.

### Endpoints to block

| Hostname | What it serves |
| --- | --- |
| `open-vsx.org`, `*.open-vsx.org` | Extension marketplace |
| `marketplace.visualstudio.com` | Microsoft marketplace |
| `*.gallery.vsassets.io`, `*.gallerycdn.vsassets.io` | Microsoft marketplace CDN |
| `github.com`, `api.github.com` | Copilot authentication |
| `copilot-proxy.githubusercontent.com` | Copilot suggestion API |
| `*.githubcopilot.com` | Copilot API (all plan tiers) |
| `copilot-telemetry.githubusercontent.com` | Copilot telemetry |
| `collector.github.com` | GitHub analytics |
| `update.code.visualstudio.com` | VS Code update checks |
| `vscode.download.prss.microsoft.com` | VS Code update downloads |
| `dc.services.visualstudio.com` | Application Insights telemetry |
| `default.exp-tas.com` | VS Code experimentation service |
| `vscode-sync.trafficmanager.net`, `vscode-sync-insiders.trafficmanager.net` | Settings sync |
| `*.vscode-cdn.net`, `*.vscode-unpkg.net` | VS Code CDN |
| `raw.githubusercontent.com` | GitHub raw file access (extensions/config) |
| `download.visualstudio.microsoft.com` | VS dependencies (C++, C# language servers) |

**Core editing functionality is unaffected by blocking all of the above.**
Git operations remain functional (local git, no remote push/pull needed for exam).

**Note:** `download.visualstudio.microsoft.com` serves C++ and C# language server
binaries that extensions download on first use. Block it to prevent extension
self-downloading; ensure needed language server binaries are pre-installed in the image.

---

## Extension Management

### Pre-installing at build time

```sh
code-server --install-extension <extension-id>
code-server --install-extension /path/to/extension.vsix
```

Extensions are stored in `~/.local/share/code-server/extensions` (or
`$XDG_DATA_HOME/code-server/extensions`).

Run at image build time in the platform recipe step for `interactive`. Extensions are
baked into the image layer.

### Locking via AllowedExtensions policy

`AllowedExtensions` acts as an allowlist — extensions not listed cannot be installed or
enabled, regardless of marketplace availability. No filesystem tricks needed; the policy
enforces this at the application layer.

### assignment.yml extension keys

```yaml
editor:
  extensions:
    install: [clangd]          # pre-installed at build time; in AllowedExtensions
    allow: [vim]               # in AllowedExtensions only; student may install manually
    lock: true                 # (future use — no native mechanism yet beyond AllowedExtensions)
```

`AllowedExtensions` in `policy.json` = union of `install` + `allow`.

**Marketplace UX with AllowedExtensions:** Non-allowed extensions are **silently omitted**
from marketplace search and browse results. Students see no error and receive no
indication that extensions are being filtered — the allowlist is invisible to them.
Only extensions in the allowlist appear in search results and are installable.
This means `allow` items are naturally discoverable without any extra
communication; everything else simply doesn't appear.

---

## Terminal Access

The integrated terminal is **not an extension** — it is compiled directly into the VS Code
binary as a workbench contribution (`src/vs/workbench/contrib/terminal/`). It cannot be
removed, and no policy key or CLI flag exists to disable it.

**No `terminal.integrated.enabled` setting exists** that can be locked via policy.
The only terminal-related policy keys control chat/agent behavior, not the terminal UI.

**Option A — OS-level shell restriction (current approach):**
- Set the student container user's shell to `/usr/sbin/nologin`
- Terminal panel remains visible but any shell immediately exits
- Controlled via `editor.terminal: false` in `assignment.yml` (default: `true`)
- No patching or custom builds required

**Option B — Custom code-server patch (full removal):**
- Requires patching via the `quilt` patch system code-server already uses
- Target: `src/vs/workbench/contrib/terminal/browser/terminalPanel.ts` and related files
- Hides the terminal panel entirely from the UI
- Requires rebuilding code-server from source — significant maintenance burden; only
  worth it if the visible-but-blocked terminal causes exam integrity concerns

**Removable built-in extensions** (regular files on disk — can be deleted from the image
in the platform recipe):

| Extension | Purpose | Default action |
| --- | --- | --- |
| `tunnel-forwarding` | Forwards local ports to external URLs via VS Code tunnel | Remove |
| `simple-browser` | Opens a browser preview panel inside the editor | Remove |
| `github` | GitHub repository browsing | Remove |
| `github-authentication` | GitHub OAuth sign-in | Remove |
| `copilot` | GitHub Copilot AI (policy disables it, but remove too) | Remove |
| `terminal-suggest` | Shell command autocomplete in terminal | Remove |

These are removed from the image by default in the `interactive` platform recipe. Add
them back explicitly in `allow` if a future assignment needs them.

To remove in a recipe step:
```sh
rm -rf /usr/lib/code-server/lib/vscode/extensions/<name>
```
_(Exact base path needs verification against a built image — may differ by install method.)_

---

## Auth

`--auth none` — code-server does not enforce its own auth. Access control is the
platform's responsibility:
- Traefik routes only authenticated sessions to the container
- The platform middleware verifies the session token on every request
- SEB Browser Exam Key is verified per-request

---

## Built-in Extensions

code-server inherits all VS Code built-in extensions unchanged (VS Code is a git submodule;
code-server applies patches via `quilt` but does not modify extensions). 86 built-in
extensions ship by default, plus 5 core features compiled into the binary.

**Core features (compiled in — cannot be removed):**
- Integrated Terminal, File Explorer, Source Control, Run and Debug, Settings UI

**Extension categories:**
- Language syntax: bat, clojure, coffeescript, cpp, csharp, dart, fsharp, go, groovy, hlsl, java, julia, latex, lua, objective-c, perl, php, powershell, python, r, razor, ruby, rust, shellscript, sql, swift, typescript-basics, vb, xml, yaml
- Language servers: css-language-features, html-language-features, json-language-features, markdown-language-features, php-language-features, typescript-language-features
- Tools: docker, dotenv, emmet, npm, references-view, merge-conflict, configuration-editing
- Themes: theme-abyss, theme-defaults, theme-kimbie-dark, theme-monokai, theme-monokai-dimmed, theme-quietlight, theme-red, theme-seti, theme-solarized-dark, theme-solarized-light, theme-tomorrow-night-blue
- Auth/VCS: git, git-base, github, github-authentication, microsoft-authentication
- Remove for exams: tunnel-forwarding, simple-browser, copilot, terminal-suggest, github, github-authentication

## Open Questions

- **Removable extensions side-effects:** Verify that deleting `tunnel-forwarding`,
  `simple-browser`, `github`, `github-authentication`, `copilot`, `terminal-suggest`
  causes no breakage in code-server. Tested in `code-server-test` image — no visible
  regressions observed.
