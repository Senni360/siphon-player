---
title: Download
description: Download Encore for Windows x64, run it without an installer, and check the file.
nav_order: 1
---

Encore is built for **Windows x64** only. Each release is one ZIP file, with no
installer and no administrator password.

Get it from [the download page](https://encore.senni.dev/download), which sends
you to the newest release's ZIP, or choose a version on the
[releases page](https://github.com/Senni360/siphon-player/releases). The file is
named `siphon-vVERSION-x86_64-pc-windows-msvc.zip`.

## Run it

Extract the ZIP and open `siphon.exe`. The first time you open a new copy from
your Downloads folder, it copies itself to `%LOCALAPPDATA%\Programs\Encore`,
adds a Start menu entry, and runs from there. The download stays where it was
and can be deleted. A copy in any other folder, or one with a `siphon-data`
folder beside it, stays where it is.

To keep everything in one folder, extract the ZIP somewhere of your own, such
as `D:\Encore`. The ZIP already carries `siphon-portable.txt` beside
`siphon.exe`, so that copy stays where it is and keeps its settings, state and
caches in `siphon-data` next to the executable. Updates leave that folder alone
(see [portable copies](/settings-and-files/#portable-copies)).

SmartScreen may warn about an unknown publisher on first run; choose **More
info**, then **Run anyway**. The Windows files are not signed with a publisher
certificate, so Windows has no publisher name to show.

Encore needs OpenGL 2.0 or newer, which every current Windows graphics driver
provides. In a virtual machine or remote session that only has the Microsoft
Basic Display Adapter, it cannot open its window and says so in a message that
links to this page; install the machine's graphics driver, or enable GPU
acceleration for the virtual machine.

To choose which app opens Spotify links, use **Settings → Apps → Default apps**
in Windows. [Web links](/web-links/) covers making Encore the handler for
Spotify, SoundCloud and YouTube links too.

## Check a download

Each release publishes `checksums.txt` and its signature. To check the ZIP
against it, print its SHA-256 hash in PowerShell and compare it with the file's
line, ignoring letter case:

```powershell
Get-FileHash "$HOME\Downloads\siphon-v*-x86_64-pc-windows-msvc.zip"
```

Encore also checks each update itself before installing it, and a failed check
leaves the running copy alone.

If Windows Security flags a download, open **Windows Security → Virus &
threat protection → Protection history** and note the threat name. Include it,
and the file's hash, when you
[report the problem](https://github.com/Senni360/siphon-player/issues). To ask
Microsoft to review the file, send it through
[Submit a file for malware analysis](https://www.microsoft.com/en-us/wdsi/filesubmission)
as incorrectly detected.

## Build from source

See the [build instructions](/getting-started/#build-from-source).
