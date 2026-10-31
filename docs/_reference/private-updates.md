---
title: Private Siphon Updates
description: Update source and signing requirements for Siphon releases.
---

# Siphon updates

Siphon checks `Senni360/siphon-player` for GitHub releases once a day and
from **Check for updates**. It never falls back to the upstream
`crmne/spotifast` releases, which could replace Siphon with a different
product. The About release-notes link uses the same repository.

A preview (`0.15.0-pre.N`) is offered the next preview or the next stable
release, whichever is newer. A stable copy is offered only stable releases.

## Signing

Every update is refused unless the release's `checksums.txt` carries a valid
`checksums.txt.sig`: a raw 64-byte Ed25519 signature over the exact bytes of
`checksums.txt`, made with Siphon's release key. The public half is
`assets/update-public-key.hex`. The private half stays with the release
owner, never in this repository or a workflow log.

Since 0.15.0-pre.3 Siphon trusts its own key. 0.15.0-pre.1 and pre.2 still
carried Spotifast's inherited key, so they cannot verify Siphon's releases:
install 0.15.0-pre.3 by hand once, and later releases arrive in the app.

To sign a release's checksums before uploading them with the other assets:

```sh
openssl pkeyutl -sign -rawin -inkey update-signing-key.pem \
  -in checksums.txt -out checksums.txt.sig
```

The Windows zip must hold `siphon-portable.txt` beside `siphon.exe`, and keep
the name `siphon-vVERSION-x86_64-pc-windows-msvc.zip`; the updater looks for
both. Rotating the key means shipping the next key in
`additional_publisher_keys` first, as `fastframe-update` documents.
