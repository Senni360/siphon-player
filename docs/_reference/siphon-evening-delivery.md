---
title: Siphon Evening Delivery
description: Integration ownership and the private Windows candidate build.
nav_exclude: true
---

# Evening delivery, 30 October 2026

The owner's requested outcome is a combined, runnable Windows application by
22:00–22:30 Europe/Amsterdam. The owner should not compile anything.

Cloud backend work continues on `siphon/reliability-delivery`, based on the
previously delivered `siphon/backend-delivery` (`8b94e5b`). Six isolated workers
cover mixed queues, streaming faults, large-library performance, interrupted
persistence/account changes, friends concurrency, and independent review.
Only evidence-backed fixes enter the candidate. Cloud validation is not a claim
that real accounts, the installed helper, or the owner's audio device work.

## Integration ownership

- Claude owns the approved design and native frontend. The observed branch is
  `claude/focused-rubin-pxfk32`; its latest observed commit at the start of this
  pass was `17dd1f4`. This was a design-kit milestone, not the finished frontend.
- The local Windows agent owns cleanup, installed-helper integration and the
  final installation/launch check. Its observed published cleanup branch is
  `siphon/cleanup`, at `adb6157`. Unpublished local work is not available here.
- The cloud coordinator owns backend reliability, its integration contract,
  and the private Windows candidate build path. The owner has authorized direct
  coordination and does not want to relay branch names or babysit integration.
  This environment has no tool for messaging the separate Claude/local chats.
  Use the existing frontend PR conversation and repository handoffs as a shared
  channel, inspect published branches, and report whether a response was actually
  received. The branches above are observations, not completion signals.

Publish final frontend and local integration branches by **20:30 Amsterdam**.
Record exact commit IDs and any remaining failures in their handoffs. This leaves
time to reconcile shared files, run checks, build Windows, and have the local
agent open the result. Do not mark an incomplete frontend or helper integration
as finished merely because a binary compiles. Preserve local uncommitted work.

## Private Windows build

`.github/workflows/windows-candidate.yml` runs on a deliberate push to
`siphon/windows-candidate`, or a manual dispatch after GitHub registers the
workflow. It does not create a tag, release, deployment or installer.

The Windows x64 job runs both test configurations and a native credential-store
round trip with dummy grants, then builds `siphon.exe` in release mode. It checks
the PE architecture, static MSVC runtime and executable version. The artifact
contains the executable, portable marker, license, short launch instructions,
SHA-256 and a manifest identifying the exact source commit. It is unsigned.

An early candidate is useful to expose Windows compilation failures. It is not
the final combined app until the manifest's commit contains the accepted
frontend, cleanup/helper work and backend fixes. The local agent must fetch that
artifact, retain the installed helper, and verify actual launch, sign-in,
three-source playback, seek/next, private links and restart persistence. Cloud
CI cannot replace those local checks. GitHub runner queue time is external to
this session; report delays honestly rather than changing the promised result.

The ordinary release pipeline signs and publishes releases and is intentionally
not used for these private candidates.

## Extra AI model

The owner offered access to a model called "Jeff". Its provider/API is still
unidentified and no requests have been made to it. Additional generated test
cases or independent failure analysis may be useful once its identity is known.
No API key is needed for the current deterministic reliability work. If an
integration is justified, keep credentials in the supported environment secret
configuration, not chat, source files or build artifacts.
