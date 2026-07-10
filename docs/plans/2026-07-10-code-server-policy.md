# code-server: Wire up file-based policy enforcement

## Rationale

code-server's server path hardcodes `NullPolicyService` and the browser workbench
uses `AccountPolicyService` (GitHub account / Copilot enterprise). Neither reads
`/etc/vscode/policy.json`. The infrastructure (`FilePolicyService`,
`LINUX_SYSTEM_POLICY_FILE_PATH`) exists in VS Code and is wired up for the desktop
build but not the server/web path.

Without a working policy service:
- Students can override `telemetryLevel`, `update.mode`, and other APPLICATION-scoped
  settings from `Machine/settings.json` by writing to `User/settings.json`
- `chat.disableAIFeatures` has no policy definition, so it cannot be policy-enforced
  at all
- The `AllowedExtensions` policy (which restricts the extension marketplace) is silently
  ignored

## Alternatives Considered

- **MACHINE scope only** (our earlier patch): Changes where the UI *saves* a setting but
  does not prevent the configuration service from merging in the user layer value.
  Insufficient — students can still bypass via direct JSON edit.
- **Patching the configuration merge order**: Hacky; would need to special-case machine
  settings in multiple places. Policy system is the designed solution.
- **Network-layer blocking**: Needed anyway for extension marketplace, but doesn't cover
  in-process AI feature flags.

## Open Questions

- None — all policy names (`TelemetryLevel`, `UpdateMode`, `AllowedExtensions`,
  `ChatPluginsEnabled`, `ChatAgentMode`, `EnableFeedback`, `DisableAIFeatures`) are
  verified present in the source.

## Rollback Plan

Remove `server-policy.diff` from `patches/series`, rebuild. The container falls back to
`NullPolicyService` / `AccountPolicyService`, same behaviour as before this change.

## Implementation

### Quilt patch: `patches/server-policy.diff`

Five files changed inside `lib/vscode/`:

**1. `server/node/serverServices.ts`**
- Add imports: `URI`, `FilePolicyService`, `LINUX_SYSTEM_POLICY_FILE_PATH`
- Remove `NullPolicyService` import
- Replace `new NullPolicyService()` with `FilePolicyService('/etc/vscode/policy.json')`
  so the server-side extension host and configuration service respect policies

**2. `server/node/webClientServer.ts`**
- Import `LINUX_SYSTEM_POLICY_FILE_PATH`
- At page-render time, attempt to read `/etc/vscode/policy.json`; if present, embed
  the parsed JSON as `policies` in `WORKBENCH_WEB_CONFIGURATION` (the JSON blob
  written into the served HTML)

**3. `workbench/browser/web.api.ts`**
- Add `readonly policies?: Record<string, string | boolean | number>` to
  `IWorkbenchConstructionOptions` so the browser workbench can receive policy data

**4. `workbench/browser/web.main.ts`**
- Add imports: `AbstractPolicyService`, `PolicyDefinition`, `PolicyValue`,
  `IStringDictionary`, `MultiplexPolicyService`
- Add local class `ServerFilePolicyService extends AbstractPolicyService` that populates
  from the static `Record<string, PolicyValue>` embedded in the page
- Wire it into a `MultiplexPolicyService([serverFilePolicyService, accountPolicyService])`
  so file policies take effect first, with account policies able to add more on top

**5. `workbench/contrib/chat/browser/chat.shared.contribution.ts`**
- Add `policy: { name: 'DisableAIFeatures', category: PolicyCategory.InteractiveSession,
  minimumVersion: '4.127.0' }` to `chat.disableAIFeatures`
- This patch is applied on top of `machine-scope-settings.diff` (which already changed
  the scope to MACHINE); the context in the diff reflects the post-patch state

### Container changes

- `images/code-server-test/policy.json`: add `"DisableAIFeatures": true`
- `images/code-server-test/Containerfile`: `COPY policy.json /etc/vscode/policy.json`

### Build

Commit patch + container changes → push fork → trigger CI (linux-arm64) → download
new deb → rebuild container image.
