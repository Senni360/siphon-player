---
title: Encore on Your Phone
description: Link your phone to Encore on your computer, and use the same account, library and groups from it.
nav_order: 5
---

Encore on your phone is a web page, at `encore.senni.dev`. The page itself is
public, but it shows nothing until your phone is linked to your account. Every
request it makes after that needs that phone's own secret.

## Link your phone

1. On your computer, open Encore. Go to **Settings**, then **Devices**, then **Your phone**, and press
   **Link a phone**. Each code works once, so make another for a second phone or browser.
2. On your phone, open the link Encore shows, or open `encore.senni.dev/link#CODE`
   with the eight letters in place of `CODE`. The code stays in the part of the
   address a browser never sends, and the page clears it once the phone is linked.
   Typing the code at `encore.senni.dev` works too.

Your phone is then another device on your account, with the same groups. It shows
your library, searches Spotify, SoundCloud and YouTube, opens artist pages, and plays
on your computer. Encore on your computer has to be open, even if only in the tray.

A linked phone appears in **Settings** under **Devices > Your phone**, where **Sign out** removes
it without signing out anything else. If a phone is removed, opening it again says
so and offers to link it once more.

To keep it like an app, install it. On an iPhone, press **Share**, then **Add to
Home Screen**; Encore shows that once, and you can put the note away for good. On
Android, press **Install** when Encore offers it. Installed, it opens full screen,
keeps its place in your home screen's search and library shortcuts, and asks the
browser to keep its sign-in.

## Without a connection

The page itself opens without a connection, because its shell is stored on the
phone. Nothing plays until the connection is back, and a new version shows a small
**Refresh** note when it is ready rather than reloading under what is playing.

## With the computer off

Sign in to Spotify on your phone too, and your Spotify library still works with the
computer off. The sound then stays in Spotify: whatever the Spotify app on the phone
or another device is playing shows here, and its buttons work.

This uses your own Spotify app, the same one as on your computer. In the Spotify
developer dashboard, add this redirect address to it once:
`https://encore.senni.dev/callback`

## What it can't do

- SoundCloud, YouTube and a Spotify song Encore finds elsewhere all need Encore on
  your computer to find the audio. The phone plays everything on the page itself and
  never opens another app: a song with no match says so, with **Skip** and **Play on
  computer**.
- The first time you open it, it can take up to 15 seconds to reach your computer.
