# Build Order

How `hammer` turns an assignment and its recipes into an image, in the order things
run. Use this to reason about why a step sees (or doesn't see) something, and to debug
recipes.

## Phase order

Every assignment image is built in this order. Each phase is one or more `RUN` layers.

| # | Phase | Runs | Source |
| --- | --- | --- | --- |
| 1 | **BASE** | OS package-manager init, `student` user, `vend` helper | OS recipe `hooks.build.init` |
| 2 | **PLATFORM** | Platform install (e.g. code-server, extensions, policy) | Platform recipe `hooks.build.steps` |
| 3 | **TOOLCHAIN** | Each toolchain in the order listed, each followed by its addons | Toolchain recipes `steps` |
| 4 | **ASSIGNMENT** | The assignment's own build steps | Assignment `environment.steps` |
| 5 | **OS CLEANUP** | Package caches and other build leftovers removed | OS recipe `hooks.build.cleanup` |
| — | **RUNTIME** | `EXPOSE` ports, `USER student`, exec-form `ENTRYPOINT` | Platform recipe `entrypoint` / `ports` |

Phases with no steps are skipped.

### Why this order

- **BASE first:** everything else assumes the [OS init contract](ARCHITECTURE.md#os-init-contract):
  a working package manager, the `student` user and workspace, `curl`,
  `ca-certificates`, and `vend`.
- **PLATFORM before TOOLCHAIN:** platform steps may only rely on the OS contract, never
  on a toolchain. Toolchains can rely on the platform being installed.
- **Addons right after their parent:** an addon (e.g. `gdb` for `gcc`) may assume its
  parent is already installed.
- **ASSIGNMENT after toolchains:** assignment steps can use every compiler and tool the
  assignment selected.
- **OS CLEANUP last:** it sweeps up after every earlier phase, so caches from any phase
  don't end up in the image.

### Pre-built base images

By default an assignment builds `FROM` the pre-built base image (e.g.
`pedagog/ubuntu:22`, built with `hammer plan --os ubuntu-22`), which already contains
phase 1, so BASE is left out. `hammer plan --show-base` emits a self-contained build
`FROM` the upstream image with BASE included.

`--registry` prefixes only the pre-built base image (`localhost/pedagog/ubuntu:22`).
Upstream images such as `ubuntu:22.04` are always pulled as written.

## Toolchains and addons

Every version is pinned in the assignment; there is no "latest". The assignment file is
the lockfile.

```yaml
toolchains:
  - gcc:13+gdb:14+valgrind:3      # shorthand: parent, then its addons
  - id: python                    # object form, same meaning
    version: "3.12"
    addons: [pytest:8]
```

- An addon must be one of the versions its parent recipe declares:

  ```yaml
  # toolchains/ubuntu-22/gcc/13.yaml
  addons:
    gdb: ["14"]
    valgrind: ["3"]
  ```

- The same toolchain at the same version, requested in several places (top level, or
  as an addon of more than one parent), is installed once, at its first position.

### Several versions of one toolchain

Two versions of the same toolchain in one image (e.g. `gdb:13` and `gdb:15`) are
refused unless **each** of those versions allows it, in one of two ways:

- **In its recipe**, `coexist: true` means the recipe installs side by side (e.g. into
  `/opt/<id>-<version>`). This is the preferred way.
- **In the assignment**, a trailing `!` on **every** entry for that version
  (`gcc:13+gdb:15!`, or `allow_coexist: true` in object form) means "install it anyway".
  The instructor takes responsibility: if the recipes overwrite each other, the image is
  broken.

Which coexisting version is the default on `PATH` is up to the recipes, not hammer.
The convention in the bundled recipes: install versioned binaries (`gcc-12`,
`python3.11`) and register them with `update-alternatives` at a priority derived from
the version, so the newest installed version is the default and the others stay
available by name. See `examples/compiler-comparison.yaml` and
`examples/python-versions.yaml` in the recipes repo.

When versions conflict, hammer lists each version, where it was requested, the versions
each parent allows and their overlap, and how to fix it:

```text
× toolchain `gdb` is selected at 2 versions, and not all of them allow coexisting:
    gdb:15  addon of gcc:13 (allows 14, 15)  coexist: recipe=no assignment=no
    gdb:13  addon of clang:17 (allows 13, 14)  coexist: recipe=no assignment=no
    versions allowed by every parent: 14
help: use gdb:14 everywhere, or set `coexist: true` in the gdb:15, gdb:13 recipe(s),
      or allow it in the assignment by marking every such entry: gdb:15!, gdb:13!
```

## Network

`environment.network` does **not** change the image. It is policy the platform enforces
at runtime with a Kubernetes NetworkPolicy, and it can change without a rebuild. Build
steps always have network access.

## Debugging

`hammer plan -f describe` prints the resolved order as a tree, which is the quickest way
to check what runs where:

```text
Assignment: pointers-and-memory
├── FROM ubuntu:22.04
├── BASE
│   └── OS: ubuntu-22
│       └── Step 1
├── PLATFORM
│   └── Platform: interactive
│       ├── Install code-server
│       ├── …
│       └── Configure terminal access
├── TOOLCHAIN
│   ├── Toolchain: gcc:13
│   │   ├── Install gcc 13
│   │   └── Set as default compiler
│   ├── Toolchain: gdb:14 (addon of gcc:13)
│   │   └── Install gdb
│   └── Toolchain: valgrind:3 (addon of gcc:13)
│       └── Install valgrind
├── ASSIGNMENT
│   └── Assignment: pointers-and-memory
│       └── Copy starter files
├── OS CLEANUP
│   └── OS Cleanup: ubuntu-22
│       └── Step 1
└── RUNTIME
    ├── user: student
    ├── entrypoint: /usr/bin/code-server --bind-addr 0.0.0.0:8080 …
    ├── ports: 8080
    └── network: deny by default; allow *.ubuntu.com:80,443
```

Give steps a `name:` so they're identifiable here; unnamed steps show as `Step N`.
