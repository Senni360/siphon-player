---
title: Private Siphon Updates
description: Update source and signing requirements for private app candidates.
---

# Private app updates

Siphon checks `Senni360/siphon-player` for GitHub releases. It never falls back
to the upstream `crmne/spotifast` releases, which could replace the private
app with a different product. The About release-notes link uses the same
repository.

A signed private update channel is not configured yet. Private GitHub release
checks without repository access can fail, and a candidate build has no
corresponding release page. Downloadable workflow artifacts are not updater
releases. Install a supplied candidate directly until the owner establishes
the private distribution channel.

Checksum-signature verification stays mandatory. The inherited publisher key
has not been replaced or disabled; a release owner must arrange the correct
signing identity and deliberate key rotation before enabling private updates.
This change creates no signing keys, credentials, tags or published releases.
