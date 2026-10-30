---
layout: home
title: Siphon
description: A fast, lightweight Spotify app for Linux, macOS, and Windows.
permalink: /
hero:
  name: Siphon
  text: Spotify, native and fast
  tagline: A lightweight Spotify app for Linux, macOS, and Windows. Listen on your computer, browse your music, and control your other devices.
  actions:
    - theme: brand
      text: Download
      link: /download/
    - theme: alt
      text: What is Siphon?
      link: /what-is-spotifast/
    - theme: alt
      text: GitHub
      link: https://github.com/crmne/spotifast
  image:
    src: /screenshot.png
    alt: "Siphon Home with the playlist library, recommendations, queue, and player visible"
    width: 2018
    height: 1198



<video class="hero-film image-src" controls muted loop playsinline preload="metadata" poster="/assets/images/launch-film-poster.jpg" aria-label="Siphon in a minute: a film of the app, made with its built-in demo content" hidden>
  <source src="/assets/videos/launch-film.mp4" type="video/mp4">
</video>

<script>
  // The theme's hero takes a picture; the film takes its place where
  // scripts run, and the screenshot stays where they do not.
  (function () {
    var film = document.querySelector(".hero-film");
    var slot = document.querySelector(".VPHero .image-container");
    if (!film || !slot) return;
    slot.replaceChildren(film);
    film.hidden = false;
    if (!window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
      film.play().catch(function () {});
    }
  })();
</script>

<style>
  .hero-film {
    display: block;
    background: #0b0e0c;
  }
</style>

## It turns into Winamp

Load a classic `.wsz` skin from the
[Winamp Skin Museum](https://skins.webamp.org). The mini player includes
animated sound displays, an equalizer to adjust your sound, and a playlist.
Roll it up into a thin bar or enlarge it while keeping the classic pixels
sharp. [See the mini player in detail](/winamp/).

<div class="winamp-showcase">
  <img src="/assets/images/winamp.png" alt="The mini player wearing the built-in skin" width="550" height="812">
</div>







## WhatsApp, just as fast

**Want WhatsApp just as fast and native?** [ZapFast](https://zapfast.rocks) is
Siphon's sibling: the same native interface, for WhatsApp. Both are built on [fastframe](https://fastframe.dev), the shared
foundation for native Rust apps built with [egui](https://www.egui.rs).
