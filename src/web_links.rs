//! Encore as the desktop's handler for web links.
//!
//! When someone picks Encore for HTTP and HTTPS in Windows, every link clicked
//! outside a browser arrives here as `--open-url <url>`. A music link becomes
//! the normal link launch; everything else goes on to the browser they used
//! before, before any part of the app starts. See
//! `docs/_reference/web-links.md`.

/// The flag Windows launches Encore with for a clicked web link.
pub const OPEN_URL_FLAG: &str = "--open-url";
/// Registers the handler and opens Encore's page in Default apps.
pub const SET_UP_FLAG: &str = "--set-up-web-links";

/// Where a clicked web link belongs.
#[derive(Debug, PartialEq, Eq)]
pub enum Route {
    /// Music: open this canonical link in Encore.
    Encore(String),
    /// A normal YouTube video: Encore only if it turns out to be a beat.
    IfBeat(String),
    Browser,
}

/// SoundCloud's own pages, which share the `/name/thing` shape of a track.
const SOUNDCLOUD_PAGES: [&str; 18] = [
    "charts",
    "connect",
    "discover",
    "imprint",
    "jobs",
    "logout",
    "messages",
    "mobile",
    "notifications",
    "oauth",
    "pages",
    "people",
    "search",
    "settings",
    "signin",
    "stream",
    "tags",
    "you",
];

/// Decides from the address alone. Only the exact host counts, so a link can
/// not reach Encore by mentioning a music service in its path or user name.
pub fn route(text: &str) -> Route {
    let Ok(url) = reqwest::Url::parse(text.trim()) else {
        return Route::Browser;
    };
    if !matches!(url.scheme(), "https" | "http")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return Route::Browser;
    }
    let music = match url.host_str() {
        Some("open.spotify.com" | "play.spotify.com") => crate::link::parse(url.as_str()),
        Some("on.soundcloud.com") => crate::link::soundcloud_url(url.as_str()),
        Some("soundcloud.com" | "www.soundcloud.com" | "m.soundcloud.com") => {
            let first = url.path_segments().and_then(|mut parts| parts.next());
            if first.is_none_or(|first| SOUNDCLOUD_PAGES.contains(&first)) {
                None
            } else {
                let mut secure = url.clone();
                let _ = secure.set_scheme("https");
                crate::link::playlist_item(secure.as_str())
            }
        }
        Some("music.youtube.com") => crate::youtube::parse(url.as_str()),
        Some("youtube.com" | "www.youtube.com" | "m.youtube.com" | "youtu.be" | "www.youtu.be") => {
            return crate::youtube::parse(url.as_str()).map_or(Route::Browser, Route::IfBeat);
        }
        _ => None,
    };
    music.map_or(Route::Browser, Route::Encore)
}

/// Words that make a video about beats rather than a beat.
const NOT_A_BEAT: [&str; 16] = [
    "beat saber",
    "breakdown",
    "cook up",
    "cookup",
    "explained",
    "how i",
    "how to",
    "interview",
    "karaoke",
    "lesson",
    "making",
    "metronome",
    "podcast",
    "reaction",
    "review",
    "tutorial",
];

/// Words that name a beat wherever they appear in a title.
const BEAT_WORDS: [&str; 6] = [
    "type beat",
    "typebeat",
    "free beat",
    "instrumental",
    "instrumentals",
    "prod",
];

/// Words that confirm a title's plain "beat" is one you can listen to.
const BEAT_COMPANIONS: [&str; 3] = ["bpm", "free", "key"];

/// Lower-cased words between single spaces, so a phrase can be matched as whole
/// words: "(Prod. X)" holds " prod ", "Product" does not.
fn words(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push(' ');
    for word in text.split(|c: char| !c.is_alphanumeric()) {
        if !word.is_empty() {
            out.extend(word.chars().flat_map(char::to_lowercase));
            out.push(' ');
        }
    }
    out
}

fn holds(haystack: &str, needle: &str) -> bool {
    haystack.contains(&format!(" {needle} "))
}

/// Whether a video's title and channel read like a beat. YouTube has no such
/// label, so this is a judgement that prefers leaving a beat in the browser to
/// sending an unrelated video to a music player.
pub fn is_beat(title: &str, channel: &str) -> bool {
    let title = words(title);
    if NOT_A_BEAT.iter().any(|word| holds(&title, word)) {
        return false;
    }
    if BEAT_WORDS.iter().any(|word| holds(&title, word)) {
        return true;
    }
    if (holds(&title, "beat") || holds(&title, "beats"))
        && (BEAT_COMPANIONS.iter().any(|word| holds(&title, word)) || has_tempo(&title))
    {
        return true;
    }
    // "Someone Beats" and "prod. someone" are beat channels; "Beats by Dre" is not.
    let channel = words(channel);
    let channel = channel.trim();
    let mut names = channel.split(' ');
    let first = names.next().unwrap_or_default();
    let last = names.next_back().unwrap_or_default();
    matches!(last, "beats" | "instrumentals") || (first == "prod" && !last.is_empty())
}

/// "140bpm" arrives as one word.
fn has_tempo(title: &str) -> bool {
    title.split(' ').any(|word| {
        word.strip_suffix("bpm")
            .is_some_and(|tempo| !tempo.is_empty() && tempo.bytes().all(|b| b.is_ascii_digit()))
    })
}

/// A browser's open command, split into its program and the arguments that
/// carry `url`. Windows writes these as `"C:\path\browser.exe" --flag "%1"`.
pub fn browser_command(command: &str, url: &str) -> Option<(String, String)> {
    let command = command.trim();
    let (program, rest) = if let Some(quoted) = command.strip_prefix('"') {
        quoted.split_once('"')?
    } else {
        command
            .split_once(char::is_whitespace)
            .unwrap_or((command, ""))
    };
    if program.is_empty() {
        return None;
    }
    let rest = rest.trim();
    let arguments = ["%1", "%l", "%L"]
        .iter()
        .find(|slot| rest.contains(**slot))
        .map(|slot| rest.replacen(slot, url, 1))
        .unwrap_or_else(|| format!("{rest} \"{url}\""));
    Some((
        program.to_owned(),
        arguments.replace("%*", "").trim().to_owned(),
    ))
}

/// The address to hand on, normalised so it cannot break out of the browser's
/// command line: the URL parser percent-encodes spaces and double quotes.
pub fn safe_web_url(text: &str) -> Option<String> {
    let url = reqwest::Url::parse(text.trim()).ok()?;
    matches!(url.scheme(), "https" | "http").then(|| url.to_string())
}

/// Whether another program's handler for `spotify:` should give way to this
/// one: nothing is registered, its program is gone, or it is an older copy of
/// this player. Spotify's own app keeps what it registered.
pub fn should_claim_spotify(command: Option<&str>, program_exists: impl Fn(&str) -> bool) -> bool {
    let Some(command) = command else {
        return true;
    };
    let Some((program, _)) = browser_command(command, "") else {
        return true;
    };
    let name = program
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(name.as_str(), "siphon.exe" | "spotifast.exe") || !program_exists(&program)
}

#[cfg(windows)]
pub use windows::{intercept, open_in_browser, register, replaces_browser, set_up};

#[cfg(windows)]
mod windows {
    use std::ffi::OsString;
    use std::io;
    use std::os::windows::process::CommandExt;
    use std::time::Duration;

    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CLASSES_ROOT, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ,
        RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ, RegCloseKey, RegCreateKeyExW, RegGetValueW,
        RegSetValueExW,
    };

    use super::{OPEN_URL_FLAG, Route, SET_UP_FLAG};

    const PROG_ID: &str = "Encore.WebLink";
    const BROWSER_KEY: &str = "Software\\Clients\\StartMenuInternet\\Encore";
    /// Beside `Exe`, which the extension bridge publishes.
    const PLAYER_KEY: &str = "Software\\Siphon Player";
    const REMEMBERED_BROWSER: &str = "WebLinkBrowser";
    /// A click is waiting on this, so a slow answer counts as "not a beat".
    const LOOKUP_TIMEOUT: Duration = Duration::from_millis(1200);

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(Some(0)).collect()
    }

    fn read(root: HKEY, key: &str, name: Option<&str>) -> Option<String> {
        let key = wide(key);
        let name = name.map(wide);
        let name_ptr = name.as_ref().map_or(std::ptr::null(), |name| name.as_ptr());
        let flags = RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ;
        let mut bytes = 0u32;
        // SAFETY: Strings are NUL-terminated and outlive the calls; the buffer
        // is sized in bytes from the first call and never read past `bytes`.
        unsafe {
            if RegGetValueW(
                root,
                key.as_ptr(),
                name_ptr,
                flags,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut bytes,
            ) != ERROR_SUCCESS
            {
                return None;
            }
            let mut buffer = vec![0u16; (bytes as usize).div_ceil(2)];
            if RegGetValueW(
                root,
                key.as_ptr(),
                name_ptr,
                flags,
                std::ptr::null_mut(),
                buffer.as_mut_ptr().cast(),
                &mut bytes,
            ) != ERROR_SUCCESS
            {
                return None;
            }
            let end = buffer
                .iter()
                .position(|unit| *unit == 0)
                .unwrap_or(buffer.len());
            Some(String::from_utf16_lossy(&buffer[..end]))
        }
    }

    fn write(key: &str, name: Option<&str>, value: &str) {
        let key = wide(key);
        let name = name.map(wide);
        let value = wide(value);
        let mut handle = std::ptr::null_mut();
        // SAFETY: Strings are NUL-terminated and outlive the calls; an opened
        // handle is closed on this thread.
        unsafe {
            if RegCreateKeyExW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                std::ptr::null(),
                &mut handle,
                std::ptr::null_mut(),
            ) != ERROR_SUCCESS
            {
                return;
            }
            let _ = RegSetValueExW(
                handle,
                name.as_ref().map_or(std::ptr::null(), |name| name.as_ptr()),
                0,
                REG_SZ,
                value.as_ptr().cast(),
                std::mem::size_of_val(value.as_slice()) as u32,
            );
            RegCloseKey(handle);
        }
    }

    /// Asks Windows rather than reading `UserChoice`: newer builds keep a second
    /// record, `UserChoiceLatest`, and the two can disagree.
    fn current_choice() -> Option<String> {
        use windows_sys::Win32::UI::Shell::{
            ASSOCF_IS_PROTOCOL, ASSOCSTR_PROGID, AssocQueryStringW,
        };
        let scheme = wide("https");
        let mut buffer = [0u16; 512];
        let mut length = buffer.len() as u32;
        // SAFETY: The scheme is NUL-terminated, and `length` holds the buffer's
        // size in characters, which the call does not write past.
        let result = unsafe {
            AssocQueryStringW(
                ASSOCF_IS_PROTOCOL,
                ASSOCSTR_PROGID,
                scheme.as_ptr(),
                std::ptr::null(),
                buffer.as_mut_ptr(),
                &mut length,
            )
        };
        if result != 0 {
            return None;
        }
        let end = buffer.iter().position(|unit| *unit == 0)?;
        (end > 0).then(|| String::from_utf16_lossy(&buffer[..end]))
    }

    /// True once someone has picked Encore for web links, when Encore's own
    /// "open in browser" must skip the handler or it would come straight back.
    pub fn replaces_browser() -> bool {
        current_choice().as_deref() == Some(PROG_ID)
    }

    /// Publishes this copy as a handler Windows can offer. Picking it stays with
    /// the person: Windows protects that choice from programs. The launch also
    /// keeps the commands on the copy now running, so replacing a portable
    /// folder cannot leave links pointing at a deleted one.
    pub fn register(exe: &std::path::Path) {
        let exe = exe.display();
        let command = format!("\"{exe}\" {OPEN_URL_FLAG} \"%1\"");
        let icon = format!("\"{exe}\",0");
        let class = format!("Software\\Classes\\{PROG_ID}");
        write(&class, None, "Encore web link");
        write(&format!("{class}\\DefaultIcon"), None, &icon);
        write(&format!("{class}\\shell\\open\\command"), None, &command);

        let capabilities = format!("{BROWSER_KEY}\\Capabilities");
        write(BROWSER_KEY, None, "Encore");
        write(&format!("{BROWSER_KEY}\\DefaultIcon"), None, &icon);
        write(
            &format!("{BROWSER_KEY}\\shell\\open\\command"),
            None,
            &format!("\"{exe}\""),
        );
        write(&capabilities, Some("ApplicationName"), "Encore");
        write(&capabilities, Some("ApplicationIcon"), &icon);
        write(
            &capabilities,
            Some("ApplicationDescription"),
            "Opens music links in Encore and everything else in your browser",
        );
        let urls = format!("{capabilities}\\URLAssociations");
        write(&urls, Some("http"), PROG_ID);
        write(&urls, Some("https"), PROG_ID);
        write(
            "Software\\RegisteredApplications",
            Some("Encore"),
            &capabilities,
        );

        if let Some(browser) = current_choice().filter(|id| id != PROG_ID) {
            write(PLAYER_KEY, Some(REMEMBERED_BROWSER), &browser);
        }
        claim_spotify(&format!("\"{exe}\" \"%1\""));
    }

    fn claim_spotify(command: &str) {
        const OPEN: &str = "Software\\Classes\\spotify\\shell\\open\\command";
        let current = read(HKEY_CURRENT_USER, OPEN, None);
        if super::should_claim_spotify(current.as_deref(), |program| {
            std::path::Path::new(program).is_file()
        }) {
            write("Software\\Classes\\spotify", None, "URL:Spotify link");
            write("Software\\Classes\\spotify", Some("URL Protocol"), "");
            write(OPEN, None, command);
        }
    }

    /// Takes a web-link launch out of the arguments. Returns the arguments for
    /// the normal launch when the link is music, and otherwise ends the process.
    pub fn intercept(arguments: Vec<OsString>) -> Vec<OsString> {
        match arguments.get(1).and_then(|flag| flag.to_str()) {
            Some(OPEN_URL_FLAG) => {
                let url = arguments
                    .get(2)
                    .map(|url| url.to_string_lossy().into_owned());
                match url.as_deref().map(destination) {
                    Some(Some(link)) => vec![arguments[0].clone(), link.into()],
                    Some(None) => {
                        let opened = url
                            .as_deref()
                            .is_some_and(|url| open_in_browser(url).is_ok());
                        std::process::exit(if opened { 0 } else { 1 });
                    }
                    None => std::process::exit(2),
                }
            }
            Some(SET_UP_FLAG) => std::process::exit(if set_up().is_ok() { 0 } else { 1 }),
            _ => arguments,
        }
    }

    /// The link to open in Encore, or `None` for the browser. YouTube links need
    /// the helper that plays them, so without it they all stay in the browser.
    fn destination(url: &str) -> Option<String> {
        match super::route(url) {
            Route::Browser => None,
            Route::Encore(link) => (!crate::youtube::is_track_uri(&link)
                || crate::youtube::Resolver::default().is_installed())
            .then_some(link),
            Route::IfBeat(link) => {
                if !crate::youtube::Resolver::default().is_installed() {
                    return None;
                }
                let (title, channel) = video_names(&link)?;
                super::is_beat(&title, &channel).then_some(link)
            }
        }
    }

    /// YouTube's public oEmbed answer: a title and channel, with no account.
    fn video_names(uri: &str) -> Option<(String, String)> {
        #[derive(serde::Deserialize)]
        struct Oembed {
            #[serde(default)]
            title: String,
            #[serde(default)]
            author_name: String,
        }
        let mut lookup = reqwest::Url::parse("https://www.youtube.com/oembed").ok()?;
        lookup
            .query_pairs_mut()
            .append_pair("format", "json")
            .append_pair("url", &crate::youtube::permalink(uri)?);
        let answer: Oembed = reqwest::blocking::Client::builder()
            .timeout(LOOKUP_TIMEOUT)
            .build()
            .ok()?
            .get(lookup)
            .send()
            .ok()?
            .error_for_status()
            .ok()?
            .json()
            .ok()?;
        Some((answer.title, answer.author_name))
    }

    /// Reaches YouTube, so it runs only when asked:
    /// `cargo test --lib web_links::windows::live_video_names -- --ignored`.
    #[cfg(test)]
    #[test]
    #[ignore = "reaches YouTube"]
    fn live_video_names() {
        let started = std::time::Instant::now();
        let (title, channel) = video_names("youtube:video:dQw4w9WgXcQ").expect("an answer");
        eprintln!("{:?}: {title} / {channel}", started.elapsed());
        assert!(title.contains("Never Gonna Give You Up"), "{title}");
        assert_eq!(channel, "Rick Astley");
        assert!(!super::is_beat(&title, &channel));
    }

    /// Opens a web address in the browser, never in this handler. Windows' own
    /// browser is the last resort: its scheme works whatever is the default.
    pub fn open_in_browser(url: &str) -> io::Result<()> {
        let url = super::safe_web_url(url).ok_or(io::ErrorKind::InvalidInput)?;
        let exe = std::env::current_exe().ok();
        let opened = [
            read(HKEY_CURRENT_USER, PLAYER_KEY, Some(REMEMBERED_BROWSER)),
            current_choice(),
        ]
        .into_iter()
        .flatten()
        .filter(|id| id != PROG_ID)
        .filter_map(|id| {
            read(
                HKEY_CLASSES_ROOT,
                &format!("{id}\\shell\\open\\command"),
                None,
            )
        })
        .filter_map(|command| super::browser_command(&command, &url))
        .filter(|(program, _)| {
            std::path::Path::new(program).is_file()
                && exe.as_deref() != Some(std::path::Path::new(program))
        })
        .any(|(program, arguments)| launch(&program, &arguments).is_ok());
        if opened {
            return Ok(());
        }
        crate::opener::open_shell(format!("microsoft-edge:{url}"))
    }

    fn launch(program: &str, arguments: &str) -> io::Result<()> {
        use windows_sys::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow};
        // SAFETY: Takes no pointers. The click's foreground right passes to the
        // browser, whose window would otherwise only flash in the taskbar.
        unsafe { AllowSetForegroundWindow(ASFW_ANY) };
        let program = std::path::Path::new(program);
        let mut command = std::process::Command::new(program);
        if let Some(folder) = program.parent() {
            command.current_dir(folder);
        }
        command.raw_arg(arguments).spawn().map(drop)
    }

    /// Registers this copy and opens its page in Windows' Default apps, where
    /// the person makes it the handler for HTTP and HTTPS.
    pub fn set_up() -> io::Result<()> {
        use windows_sys::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
        register(&std::env::current_exe()?);
        // SAFETY: This event takes no item pointers.
        unsafe {
            SHChangeNotify(
                SHCNE_ASSOCCHANGED as i32,
                SHCNF_IDLIST,
                std::ptr::null(),
                std::ptr::null(),
            );
        }
        crate::opener::open_shell("ms-settings:defaultapps?registeredAppUser=Encore")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn music(url: &str) -> bool {
        matches!(route(url), Route::Encore(_))
    }

    #[test]
    fn music_services_open_in_encore() {
        assert_eq!(
            route("https://open.spotify.com/intl-de/track/4uLU6hMCjMI75M1A2tKUQC?si=abc"),
            Route::Encore("spotify:track:4uLU6hMCjMI75M1A2tKUQC".into())
        );
        assert_eq!(
            route("https://music.youtube.com/watch?v=dQw4w9WgXcQ&si=abc"),
            Route::Encore("youtube:video:dQw4w9WgXcQ".into())
        );
        assert!(music("https://soundcloud.com/artist/a-track"));
        assert!(music("https://soundcloud.com/artist/sets/a-set"));
        assert!(music("https://soundcloud.com/artist/a-track/s-secret"));
        assert!(music("http://m.soundcloud.com/artist/a-track"));
        assert!(music("https://on.soundcloud.com/AbCdE"));
    }

    #[test]
    fn normal_youtube_videos_wait_for_the_beat_check() {
        for url in [
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            "https://youtu.be/dQw4w9WgXcQ?t=4",
            "https://m.youtube.com/watch?v=dQw4w9WgXcQ",
            "https://www.youtube.com/shorts/dQw4w9WgXcQ",
        ] {
            assert_eq!(
                route(url),
                Route::IfBeat("youtube:video:dQw4w9WgXcQ".into()),
                "{url}"
            );
        }
    }

    #[test]
    fn everything_else_stays_in_the_browser() {
        for url in [
            "https://example.com/",
            "https://github.com/crmne/spotifast",
            // Spotify's sign-in must reach the browser or nobody can log in.
            "https://accounts.spotify.com/authorize?client_id=x",
            "https://open.spotify.com/",
            "https://open.spotify.com/user/someone",
            "https://www.youtube.com/",
            "https://www.youtube.com/@channel",
            "https://www.youtube.com/playlist?list=PL1",
            "https://music.youtube.com/playlist?list=PL1",
            "https://soundcloud.com/",
            "https://soundcloud.com/artist",
            "https://soundcloud.com/artist/likes",
            "https://soundcloud.com/signin",
            "https://soundcloud.com/oauth/authorize",
            "https://soundcloud.com/you/library",
            "https://soundcloud.com/search/sounds",
            "https://soundcloud.com/discover/sets",
            // A music host named anywhere but as the host proves nothing.
            "https://evil.example/soundcloud.com/artist/a-track",
            "https://evil.example/#https://open.spotify.com/track/4uLU6hMCjMI75M1A2tKUQC",
            "https://open.spotify.com@evil.example/track/4uLU6hMCjMI75M1A2tKUQC",
            "https://soundcloud.com.evil.example/artist/a-track",
            "https://user:pass@soundcloud.com/artist/a-track",
            "https://soundcloud.com:8443/artist/a-track",
            "file:///C:/Windows/system.ini",
            "not a link",
        ] {
            assert_eq!(route(url), Route::Browser, "{url}");
        }
    }

    #[test]
    fn beat_titles_open_in_encore() {
        for (title, channel) in [
            ("[FREE] Travis Scott Type Beat - \"Midnight\"", "someone"),
            ("Drake x Future TYPE BEAT 2026", "someone"),
            ("Dark Trap Instrumental", "someone"),
            ("Midnight (prod. Someone)", "someone"),
            ("Midnight [Prod by Someone]", "someone"),
            ("hard trap beat 140 bpm", "someone"),
            ("hard trap beat 140BPM", "someone"),
            ("free drill beat", "someone"),
            ("sad piano beat in C minor key", "someone"),
            ("Midnight", "Someone Beats"),
            ("Midnight", "Prod. Someone"),
            // Real titles from YouTube.
            (
                "[FREE] young nudy x lucki type beat - tic tac",
                "ayohighflyer",
            ),
            (
                "[FREE FOR PROFIT] Melodic Type Beat \"VIPER\"| Dark Type Beat",
                "VEEDY BEATS",
            ),
            ("[FREE] FREESTYLE RAP INSTRUMENTAL BEAT", "Mixla"),
            (
                "[FREE] Acoustic Guitar Type Beat \"On The Edge\"",
                "GAXILLIC",
            ),
        ] {
            assert!(is_beat(title, channel), "{title} / {channel}");
        }
    }

    #[test]
    fn other_videos_stay_in_the_browser() {
        for (title, channel) in [
            ("How to make a Travis Scott type beat", "someone"),
            ("FL Studio type beat tutorial", "someone"),
            ("Reacting to my beats: producer reaction", "someone"),
            ("Making a free beat in 10 minutes", "someone"),
            ("Metronome 140 BPM beat", "someone"),
            ("Beat Saber free songs", "someone"),
            ("Cat videos to beat the blues", "someone"),
            ("Beats Studio Pro review", "Beats by Dre"),
            ("Product launch", "someone"),
            ("Rick Astley - Never Gonna Give You Up", "Rick Astley"),
            ("", ""),
            // Real titles from YouTube.
            ("how to make a beat on FL STUDIO (Beginner)", "shonci"),
            (
                "Teaching a \"Noob Producer\" How To Make Beats From Scratch",
                "EdTalenti",
            ),
            (
                "Music Production For Beginners (Step By Step)",
                "Ethan Davis",
            ),
            ("Kehlani - Folded [Official Music Video]", "Kehlani"),
            ("Linkin Park - In the end (beatbox cover)", "Petr Sarancha"),
            ("IMPROVER - Feel The Future (BEATBOX)", "Improver"),
        ] {
            assert!(!is_beat(title, channel), "{title} / {channel}");
        }
    }

    #[test]
    fn a_browser_command_receives_the_link_as_one_argument() {
        let url = "https://example.com/a?b=c";
        assert_eq!(
            browser_command(r#""C:\Helium\chrome.exe" --single-argument %1"#, url),
            Some((
                r"C:\Helium\chrome.exe".into(),
                format!("--single-argument {url}")
            ))
        );
        assert_eq!(
            browser_command(r#""C:\Program Files\Fx\firefox.exe" -osint -url "%1""#, url),
            Some((
                r"C:\Program Files\Fx\firefox.exe".into(),
                format!("-osint -url \"{url}\"")
            ))
        );
        assert_eq!(
            browser_command(r"C:\b\browser.exe", url),
            Some((r"C:\b\browser.exe".into(), format!("\"{url}\"")))
        );
        assert_eq!(browser_command("\"unterminated", url), None);
        assert_eq!(browser_command("", url), None);
    }

    #[test]
    fn a_link_cannot_escape_the_browser_command() {
        let url = safe_web_url("https://example.com/a b?c=\"d\" --evil").unwrap();
        assert!(!url.contains('"') && !url.contains(' '), "{url}");
        // A link that spells the placeholder must not be substituted again.
        let url = safe_web_url("https://example.com/%1").unwrap();
        let (_, arguments) = browser_command(r#""b.exe" "%1" %*"#, &url).unwrap();
        assert_eq!(arguments, format!("\"{url}\""));
        for refused in ["file:///C:/x", "javascript:alert(1)", "ms-settings:", "x"] {
            assert_eq!(safe_web_url(refused), None, "{refused}");
        }
    }

    #[test]
    fn spotify_links_move_only_from_nothing_or_an_older_copy() {
        let exists = |_: &str| true;
        assert!(should_claim_spotify(None, exists));
        assert!(should_claim_spotify(
            Some(r#""C:\Programs\Spotifast\spotifast.exe" "%1""#),
            exists
        ));
        assert!(should_claim_spotify(
            Some(r#""D:\Downloads\old\siphon.exe" "%1""#),
            exists
        ));
        assert!(should_claim_spotify(
            Some(r#""C:\Gone\Other.exe" "%1""#),
            |_| false
        ));
        assert!(!should_claim_spotify(
            Some(r#""C:\Spotify\Spotify.exe" --protocol-uri="%1""#),
            exists
        ));
    }
}
