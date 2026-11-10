---
layout: home
title: Siphon
description: A fast, private music app for Windows, with Spotify, SoundCloud and YouTube in one library.
permalink: /
hero:
  name: Siphon
  text: Spotify, SoundCloud and YouTube, native and fast
  tagline: A private music app for Windows. One library, one queue and one search for the group's music, with a phone that works alongside it.
  actions:
    - theme: brand
      text: Download
      link: /download/
    - theme: alt
      text: What is Siphon?
      link: /what-is-spotifast/
    - theme: alt
      text: GitHub
      link: https://github.com/Senni360/siphon-player
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

## Your music, also on your phone

Link a phone with a one-time code from Settings. It then has the same account,
groups and library, and can play on the computer or on the phone itself.
[Set up the phone](/phone/).







## WhatsApp, just as fast

**Want WhatsApp just as fast and native?** [ZapFast](https://zapfast.rocks) is
Siphon's sibling: the same native interface, for WhatsApp. Both are built on [fastframe](https://fastframe.dev), the shared
foundation for native Rust apps built with [egui](https://www.egui.rs).
