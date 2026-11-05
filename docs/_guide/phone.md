---
title: Encore on Your Phone
description: Link your phone to Encore on your computer, and use the same account, library and groups from it.
nav_order: 5
---

Encore on your phone is a web page, at `encore.senni.dev`. It isn't public: it asks
for a code first, which whoever runs your group can give you.

## Link your phone

1. On your computer, open Encore. Go to **Settings**, then **Your phone**, and press
   **Link a phone**. You can also find it in **Group**, under **People**.
2. On your phone, open `encore.senni.dev`, type the code to get in, then type the
   eight letters Encore shows on your computer.

Your phone is then another device on your account, with the same groups. It shows
your library, searches Spotify, SoundCloud and YouTube, opens artist pages, and plays
on your computer. Encore on your computer has to be open, even if only in the tray.

A linked phone appears in **Settings** under **Your phone**, where **Sign out** removes
it without signing out anything else.

To keep it like an app, add the page to your home screen: on an iPhone, press **Share**,
then **Add to Home Screen**.

## With the computer off

Sign in to Spotify on your phone too, and your Spotify library still works with the
computer off. It plays on your other Spotify devices, which you choose by pressing the
line under the song in Now Playing.

This uses your own Spotify app, the same one as on your computer. In the Spotify
developer dashboard, add this redirect address to it once:
`https://encore.senni.dev/callback`

## What it can't do

- A phone's browser can't play Spotify, so the sound comes from your computer or your
  Spotify devices.
- SoundCloud and YouTube need Encore on your computer. With the computer off, a song
  shared in the group opens in its own app instead.
- The first time you open it, it can take up to 15 seconds to reach your computer.
