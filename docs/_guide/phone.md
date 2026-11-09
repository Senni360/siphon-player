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
on your computer or on the phone itself. More than one phone, or another browser on
the same phone, can be linked, each with its own code and its own sign-in.

Linked phones and browsers are listed in **Settings** under **Devices > Your phone**,
where **Sign out** removes one without signing out anything else. If a phone is
removed, opening it again says so and offers to link it once more.

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
computer off. This uses your own Spotify app, the same one as on your computer. In
the Spotify developer dashboard, add this redirect address to it once:
`https://encore.senni.dev/callback`

The page still plays the sound itself. Choose **This phone** under **Playing on**, and
a Spotify song plays as the same song found on YouTube or SoundCloud. With a Spotify
sign-in, the page can also work whatever your Spotify account is already playing on
your other devices; it never opens Spotify to start a song.

## What it can't do

- SoundCloud, YouTube and the group come from Encore on your computer, so with it off
  the page shows your Spotify library and your group. Playback itself keeps working:
  the owner's always-on server finds and fetches the audio for a public SoundCloud
  song, a YouTube video, or a Spotify song matched to one. Anything else asks your
  computer. The phone plays everything on the page itself and never opens another
  app: a song with no audio says so, with **Skip** and **Play on computer**.
- A song that stops coming is tried again at the same place, never skipped: after
  1, 2 and 4 seconds, then every 8 seconds, and after 10 seconds with no audio it
  starts the same song again. Another song starts only when this one ends or you
  choose one.
- The first time you open it, it can take up to 20 seconds to reach your computer.
