# code-server Image Hardening: Ideas Not Yet on Main

> Salvaged from the archived `feat/m1-code-server` prototype (tag `archive/m1-code-server`),
> a Wolfi + runit image. Main's lockdown (VS Code policy, patched code-server) is described in
> [code-server.md](code-server.md); this note keeps only what main doesn't already do.
> Sources: `images/base/Containerfile`, `images/base/rootfs/etc/service/code-server/run`
> (d020074, 5e89408, 313aff3, 6df017c, 4e17b21).

## Read-only extensions directory

Today the interactive platform installs extensions into
`/home/student/.local/share/code-server/extensions`, owned by `student`, so a student can add
or delete extension files directly on disk. The prototype installed into a root-owned
directory and made it read-only:

```sh
# After installing the allowed extensions at build time:
chown -R root:root /var/lib/code-server/extensions
chmod -R a=rX /var/lib/code-server/extensions
# then run code-server with: --extensions-dir /var/lib/code-server/extensions
```

This complements the `AllowedExtensions` policy: policy decides what may run, and file
permissions stop the installed set from being changed.

## Editor state outside the student's home

The prototype pointed `--user-data-dir`, `--extensions-dir`, `XDG_CACHE_HOME` and
`XDG_DATA_HOME` at `/var/lib/code-server/...`, so editor state never lands in files the
student owns, archives or submits. Main keeps the workspace (`/home/student/workspace`)
separate from state (`~/.local`), which covers submissions. It doesn't stop the student from
editing their own `settings.json` or the `Machine/settings.json` written into their home.

## Instructor sessions on the student's environment

The prototype's layout let an instructor open exactly what the student sees:

| Path | Owner:group, mode | Purpose |
| --- | --- | --- |
| `/pedagog/student` | `student:pedagogc` `2770` (setgid) | student home and work tree |
| `/pedagog/source` | `instructor:pedagog` `0750` | instructor inputs (manifest, seed files), hidden from the student |
| `/pedagog/staging` | `pedagog:pedagog` `0700` | submission packaging |
| `/pedagog/config` | `root:pedagog` `0750` | resolved config |

- Identities: `student` (uid 1001, untrusted), `pedagog` (1002, trusted broker), `instructor`
  (1003). Both `student` and `instructor` belong to the `pedagogc` group.
- Because of the setgid group on the student tree and editor-state directories, a session
  started as `instructor` (via `PEDAGOG_USER_TYPE=instructor`) can open the same folder and
  editor state as the student. Student exam sessions always run as `student`.
- Worth revisiting when instructor preview/testing of interactive assignments is designed.

## Flags main doesn't set

- `--disable-proxy`: turns off code-server's built-in port proxy (`/proxy/<port>`), so the
  student can't tunnel to other ports through the editor.
- `--disable-getting-started-override`: hides code-server's own getting-started page.

Check each against [code-server.md](code-server.md) before adopting it; policy-backed
equivalents win where they exist.

## Not carried over

- **runit as PID 1:** only useful once the container runs more than one supervised process
  (code-server plus the daemon). Revisit then.
- **nftables egress locked at boot:** superseded by NetworkPolicy
  (design-doc-v2).
