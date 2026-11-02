---
title: Web links
description: Opening music links from Discord and other apps in Encore on Windows.
nav_order: 22
---

Encore can be the Windows handler for web links, so a music link clicked in
Discord or any other app opens in Encore. Every other link goes on to your
browser. Links clicked inside a browser stay there, because the browser never
asks Windows about them.

## Turning it on

Run this once from the copy of Encore you use:

```text
siphon.exe --set-up-web-links
```

It opens Encore's page in **Settings > Apps > Default apps**. Choose Encore for
**HTTP** and **HTTPS**. Windows does not let a program make this choice itself.
To turn it off, choose your browser there again.

## Where a link goes

| Link | Opens in |
| --- | --- |
| Spotify tracks, albums, artists, playlists, shows and episodes | Encore |
| SoundCloud tracks and sets, including private and short links | Encore |
| YouTube Music videos | Encore |
| Normal YouTube videos and Shorts that look like beats | Encore |
| Other normal YouTube videos | Browser |
| SoundCloud profiles and SoundCloud's own pages | Browser |
| YouTube channels and playlists | Browser |
| Everything else, including sign-in pages | Browser |

YouTube has no beat label, so Encore judges by the title and channel. "Type
beat", "free beat", "instrumental" and "prod." count, as does "beat" alongside a
tempo, a key or "free", or a channel named "Someone Beats" or "Prod. Someone".
Words such as "tutorial", "how to", "reaction" and "review" rule a video out.
Some beats will stay in the browser, and an occasional other video may open in
Encore.

YouTube links need the Siphon helper that plays them. Without it, they all go
to the browser.

## Network

Only a normal YouTube video link causes a request of its own: the video's
address goes to YouTube's public oEmbed endpoint, without an account, for its
title and channel. If no answer arrives within 1.2 seconds, the link opens in
the browser. This one request runs before settings load, so it follows the
system proxy rather than **Settings > Proxy**. Other links are decided from the
address alone, and Encore does not record the links it hands on.

## Registry

Each launch of a release build writes these under `HKEY_CURRENT_USER`, pointing
at the running copy, so a moved or replaced copy takes the links with it once it
has started. If its folder is deleted, links fail until a new copy has started
or you choose another handler.

- `Software\Classes\Encore.WebLink`: the handler.
- `Software\Clients\StartMenuInternet\Encore` and its
  `Software\RegisteredApplications` entry: what Default apps lists.
- `WebLinkBrowser` in `Software\Siphon Player`: the browser that was the default
  before Encore, where other links go. If it is gone, Microsoft Edge opens them.
- `Software\Classes\spotify`: only when nothing handles `spotify:` links, the
  registered program no longer exists, or it is an older copy of this player.
  Spotify's own app keeps its registration.

When Encore is the handler, its own **Open in browser** and **Open in Spotify**
actions go directly to the browser.
