# code-server Release Process

Pedagog maintains a fork of code-server at **RobertConde/code-server**. Patches live on
the `pedagog/scope-patch` branch as a `quilt` patch series on top of the upstream
`main` branch.

---

## Version naming

```
v4.X.Y-pedagog.N
```

- `4.X.Y` — upstream code-server version the patches are applied on top of
- `N` — patch revision; increment when re-releasing against the same upstream version

Examples: `v4.127.0-pedagog.1`, `v4.127.0-pedagog.2`, `v4.128.0-pedagog.1`

The version string flows into the binary:
1. Workflow input `version` → `VERSION` env var (strips leading `v`)
2. `ci/build/build-vscode.sh` injects `VERSION` into `product.json` as `codeServerVersion`
3. `code-server --version` outputs: `4.127.0-pedagog.1 <commit> with Code 1.127.0`

---

## Releasing

### Prerequisites

- You are on `pedagog/scope-patch` with all patches applying cleanly (`quilt push -a`)
- The branch is pushed to `origin`

### Steps

**1. Push the branch**

```sh
git push origin pedagog/scope-patch
```

**2. Trigger the release workflow**

```sh
gh workflow run release.yaml \
  --repo RobertConde/code-server \
  --ref main \
  --field version=v4.127.0-pedagog.1
```

Omit `--field targets` to build all platforms (linux-x64, linux-arm64, darwin-x64,
darwin-arm64). To build a single platform for testing:

```sh
  --field targets=linux-arm64
```

The workflow creates a **draft** release on GitHub. Check CI status:

```sh
gh run list --repo RobertConde/code-server --workflow=release.yaml --limit 5
```

**3. Publish the release**

Once all jobs pass, publish the draft release on GitHub:

```sh
gh release edit v4.127.0-pedagog.1 \
  --repo RobertConde/code-server \
  --draft=false
```

**4. Merge into fork main**

```sh
git -C /path/to/code-server checkout main
git merge --ff-only pedagog/scope-patch
git push origin main
git checkout pedagog/scope-patch
```

---

## Vendoring the .deb into pedagog

After publishing the release, re-vendor the `.deb` for the test image:

```sh
cd images/code-server-test
just vendor
```

The Justfile resolves the latest release (including drafts) by tag and downloads the
arm64 `.deb` as `code-server.deb`.

---

## Patch maintenance

When upstream releases a new version:

1. Update the `lib/vscode` submodule to the new upstream tag
2. Run `quilt push -a` — fix any hunks that fail
3. `quilt refresh` any patches with offset warnings
4. Commit the updated submodule and any refreshed patches
5. Follow the release steps above with the new version (e.g., `v4.128.0-pedagog.1`)
