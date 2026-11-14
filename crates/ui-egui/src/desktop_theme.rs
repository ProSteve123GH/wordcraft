//! The desktop's own light/dark choice, for an interface setting that follows it.
//!
//! winit reports no system theme on Linux — `ActiveEventLoop::system_theme` is `None` on every
//! desktop there — so "Use system setting" has to ask the desktop itself. The XDG Desktop Portal's
//! Settings interface answers on GNOME, KDE and XFCE alike, over X11 and Wayland alike.
//!
//! On Android (Termux) there is no portal and no session bus to ask: the X11 settings manager
//! (`xfsettingsd` on XFCE) or, when that isn't running, the desktop's own `settings.ini` and
//! XFCE `xsettings` files answer instead (see [`xsettings_dark`] and [`settings_files_dark`]).

use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::Duration;

/// How often [`DesktopTheme::start`] re-reads the desktop's answer. The portal also publishes a
/// change signal, but subscribing needs a connection kept alive for the life of every window; one
/// round trip every few seconds is cheap and reacts soon enough after a theme switch.
const POLL: Duration = Duration::from_secs(4);

/// Whether the desktop asks applications to draw dark. `None` when it says nothing, and when
/// there is no session bus or no portal to ask (a bare X server, a container), so the caller
/// keeps whatever it chose itself.
pub fn prefers_dark() -> Option<bool> {
    read(&mut Bus::new())
}

/// Watches the desktop and reports its choice when it changes. [`prefers_dark`] covers the
/// answer at start-up; this covers the user switching theme while the app runs.
#[derive(Default)]
pub struct DesktopTheme {
    /// `None` when no watcher thread could be started; the one-shot read still works.
    changes: Option<Receiver<Option<bool>>>,
}

impl DesktopTheme {
    /// Start watching. Nothing runs before this is called, and calling it once is enough. The
    /// first answer is sent straight away, so the caller does not have to ask for it separately.
    /// `changed` runs on the watcher's thread after each answer is sent (the app asks for a
    /// repaint there, so an idle window picks the change up). The app binary starts this; tests
    /// and headless screenshots don't, so they never depend on the machine's desktop theme.
    pub fn start(changed: impl Fn() + Send + 'static) -> Self {
        let (tx, rx) = channel();
        let started = std::thread::Builder::new().name("wordcraft-desktop-theme".into()).spawn(move || {
            // One session-bus connection for the life of the watcher, opened again only after
            // it fails.
            let mut bus = Bus::new();
            let mut last = read(&mut bus);
            if tx.send(last).is_err() {
                return;
            }
            changed();
            loop {
                std::thread::sleep(POLL);
                let now = read(&mut bus);
                if now != last {
                    last = now;
                    if tx.send(now).is_err() {
                        return;
                    }
                    changed();
                }
            }
        });
        Self { changes: started.ok().map(|_| rx) }
    }

    /// Whether a watcher thread is running.
    pub fn watching(&self) -> bool {
        self.changes.is_some()
    }

    /// The desktop's new answer if it changed since the last call, to be polled once a frame.
    /// Several changes in one frame collapse to the newest, and a desktop that stops answering
    /// leaves the last answer in place.
    pub fn take_change(&mut self) -> Option<bool> {
        let Some(changes) = &self.changes else { return None };
        let mut newest = None;
        loop {
            match changes.try_recv() {
                Ok(changed) => newest = changed,
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => return newest,
            }
        }
    }
}

/// The watcher's session-bus connection, kept between polls.
#[cfg(target_os = "linux")]
struct Bus(Option<zbus::blocking::Connection>);

#[cfg(target_os = "linux")]
impl Bus {
    /// No connection yet: the first read opens one.
    fn new() -> Self {
        Bus(None)
    }
}

/// Elsewhere there is no session bus to keep.
#[cfg(not(target_os = "linux"))]
struct Bus;

#[cfg(not(target_os = "linux"))]
impl Bus {
    fn new() -> Self {
        Bus
    }
}

/// Ask the portal once, over `bus`'s connection (opened when there is none). D-Bus calls
/// block, so this belongs off the UI thread. A connection whose calls all fail is dropped, so
/// the next poll opens a fresh one (the bus or portal may have restarted).
#[cfg(target_os = "linux")]
fn read(bus: &mut Bus) -> Option<bool> {
    const PORTAL: &str = "org.freedesktop.portal.Desktop";
    const SETTINGS: &str = "org.freedesktop.portal.Settings";
    const SCHEMAS: [(&str, &str); 2] = [("org.gnome.desktop.interface", "color-scheme"), ("org.freedesktop.interface.color-scheme", "color-scheme")];
    if bus.0.is_none() {
        bus.0 = zbus::blocking::Connection::session().ok();
    }
    let connection = bus.0.as_ref()?;
    let mut answered = false;
    for (schema, key) in SCHEMAS {
        let Ok(reply) = connection.call_method(Some(PORTAL), "/org/freedesktop/portal/desktop", Some(SETTINGS), "ReadOne", &(schema, key)) else {
            continue;
        };
        answered = true;
        let Ok((answer,)) = reply.body().deserialize::<(zbus::zvariant::OwnedValue,)>() else { continue };
        if let Ok(scheme) = String::try_from(&*answer) {
            return parse(&scheme);
        }
    }
    if !answered {
        bus.0 = None;
    }
    None
}

/// The portal's `color-scheme` answer. `default` means the desktop states no preference, so
/// neither does the application.
#[cfg(target_os = "linux")]
fn parse(scheme: &str) -> Option<bool> {
    match scheme {
        "prefer-dark" => Some(true),
        "prefer-light" => Some(false),
        _ => None,
    }
}

/// Android has no portal to ask, so the X11 settings manager (XFCE's `xfsettingsd`) and the
/// desktop's own settings files answer instead. Both are read on each poll, so a theme change
/// shows without a restart.
#[cfg(target_os = "android")]
fn read(_bus: &mut Bus) -> Option<bool> {
    xsettings_dark().or_else(settings_files_dark)
}

/// macOS and Windows answer through winit, which the caller already has; the other Unixes keep
/// their own choice when no portal answers.
#[cfg(all(not(target_os = "linux"), not(target_os = "android")))]
fn read(_bus: &mut Bus) -> Option<bool> {
    None
}

/// An XSETTINGS value: the X11 settings manager's serialized properties.
#[cfg(target_os = "android")]
#[derive(Debug, PartialEq)]
enum XValue {
    Integer(i32),
    Text(String),
}

/// The value of `want` in an `_XSETTINGS_SETTINGS` blob, if it holds that setting. The format is
/// the one in the XSETTINGS specification: a one-byte byte-order marker (`l` or `B`, anything else
/// native) and padding, the setting count, then type/name/value records with four-byte alignment.
#[cfg(target_os = "android")]
fn xsettings_value(bytes: &[u8], want: &str) -> Option<XValue> {
    let big = match bytes.first()? {
        b'B' => true,
        b'l' => false,
        _ => cfg!(target_endian = "big"),
    };
    let i16_at = |at: usize| -> Option<i16> {
        let b: [u8; 2] = bytes.get(at..at + 2)?.try_into().ok()?;
        Some(if big { i16::from_be_bytes(b) } else { i16::from_le_bytes(b) })
    };
    let i32_at = |at: usize| -> Option<i32> {
        let b: [u8; 4] = bytes.get(at..at + 4)?.try_into().ok()?;
        Some(if big { i32::from_be_bytes(b) } else { i32::from_le_bytes(b) })
    };
    let mut at = 8; // byte order, three padding bytes and the serial
    let count = i32_at(at)?.max(0) as usize;
    at += 4;
    for _ in 0..count {
        let ty = *bytes.get(at)?;
        at += 2; // type and one padding byte
        let name_len = i16_at(at)?.max(0) as usize;
        at += 2;
        let name = bytes.get(at..at + name_len)?;
        at = (at + name_len + 3) & !3;
        at += 4; // the setting's serial
        let value = match ty {
            0 => {
                let n = i32_at(at)?;
                at += 4;
                XValue::Integer(n)
            }
            1 => {
                let len = i32_at(at)?.max(0) as usize;
                at += 4;
                let text = bytes.get(at..at + len)?;
                at = (at + len + 3) & !3;
                XValue::Text(String::from_utf8_lossy(text).into_owned())
            }
            2 => {
                at += 8; // four 16-bit colour components
                continue;
            }
            _ => return None,
        };
        if name == want.as_bytes() {
            return Some(value);
        }
    }
    None
}

/// Whether the X11 settings manager asks for a dark interface: GTK's prefer-dark switch, else a
/// theme name that says so. `None` when no manager is running (a bare X server).
#[cfg(target_os = "android")]
fn xsettings_dark() -> Option<bool> {
    use x11rb::protocol::xproto::ConnectionExt;

    let (connection, screen) = x11rb::connect(None).ok()?;
    let intern = |name: &str| -> Option<u32> { Some(connection.intern_atom(false, name.as_bytes()).ok()?.reply().ok()?.atom) };
    let selection = intern(&format!("_XSETTINGS_S{screen}"))?;
    // The manager owns `_XSETTINGS_S<screen>` as a selection (not a property of the root) and
    // publishes its settings on the owner window; winit reads it the same way for DPI.
    let owner = connection.get_selection_owner(selection).ok()?.reply().ok()?.owner;
    if owner == x11rb::NONE {
        return None;
    }
    let settings = intern("_XSETTINGS_SETTINGS")?;
    let property = connection.get_property(false, owner, settings, settings, 0, 1 << 20).ok()?.reply().ok()?;
    if property.format != 8 {
        return None;
    }
    let theme = match xsettings_value(&property.value, "Net/ThemeName") {
        Some(XValue::Text(name)) => Some(name),
        _ => None,
    };
    let prefer = match xsettings_value(&property.value, "Gtk/ApplicationPreferDarkTheme") {
        Some(XValue::Integer(on)) => Some(on),
        _ => None,
    };
    prefer_dark_answer(theme.as_deref(), prefer)
}

/// GTK's explicit "prefer dark" switch selects the theme's dark variant, so it wins over the
/// name (Adwaita draws dark even as "Adwaita" while the switch is on). Without the switch the
/// name decides; with neither, the desktop says nothing.
#[cfg(target_os = "android")]
fn prefer_dark_answer(theme_name: Option<&str>, prefer: Option<i32>) -> Option<bool> {
    match prefer {
        Some(on) if on != 0 => Some(true),
        _ => theme_name.map(is_dark_name),
    }
}

/// Whether the user's own settings files ask for dark, for when no settings manager is running:
/// GTK's `settings.ini` (that is what its switch lives in) and XFCE's stored `xsettings` channel
/// (its settings editor writes it within a couple of seconds of a change).
#[cfg(target_os = "android")]
fn settings_files_dark() -> Option<bool> {
    gtk_settings_dark().or_else(xfce_theme_dark)
}

/// The directory desktop settings files live in: `$XDG_CONFIG_HOME`, else `~/.config`.
#[cfg(target_os = "android")]
fn settings_root() -> Option<std::path::PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|p| !p.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").filter(|p| !p.is_empty()).map(|h| std::path::PathBuf::from(h).join(".config")))
}

/// GTK 4's or GTK 3's `settings.ini`, the first that answers.
#[cfg(target_os = "android")]
fn gtk_settings_dark() -> Option<bool> {
    let root = settings_root()?;
    for version in ["gtk-4.0", "gtk-3.0"] {
        let Ok(text) = std::fs::read_to_string(root.join(version).join("settings.ini")) else { continue };
        if let Some(on) = ini_value(&text, "Settings", "gtk-application-prefer-dark-theme") {
            return Some(matches!(on.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"));
        }
        if let Some(name) = ini_value(&text, "Settings", "gtk-theme-name") {
            return Some(is_dark_name(name));
        }
    }
    None
}

/// The `Net/ThemeName` in XFCE's `xsettings.xml`, when the settings editor has stored one.
#[cfg(target_os = "android")]
fn xfce_theme_dark() -> Option<bool> {
    let text = std::fs::read_to_string(settings_root()?.join("xfce4/xfconf/xfce-perchannel-xml/xsettings.xml")).ok()?;
    Some(is_dark_name(xml_value(&text, "ThemeName")?))
}

/// A value in an INI file's section, the last one winning, as the toolkits read it.
#[cfg(target_os = "android")]
fn ini_value<'a>(text: &'a str, section: &str, key: &str) -> Option<&'a str> {
    let mut inside = false;
    let mut found = None;
    for line in text.lines().map(str::trim) {
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            inside = name.trim() == section;
        } else if inside
            && !line.starts_with(['#', ';'])
            && let Some((k, v)) = line.split_once('=')
            && k.trim() == key
        {
            found = Some(v.trim());
        }
    }
    found
}

/// The `value="..."` of the `<property name="`name`">` element in XFCE's XML. An empty property
/// (`type="empty"`) has none.
#[cfg(target_os = "android")]
fn xml_value<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let start = text.find(&format!("name=\"{name}\""))?;
    let tag_end = start + text[start..].find('>')?;
    let tag = &text[start..tag_end];
    let after = &tag[tag.find("value=\"")? + 7..];
    Some(&after[..after.find('"')?])
}

/// A theme name that asks for a dark interface; XFCE names its dark variants that way
/// (`Adwaita-dark`, `Greybird-dark`, `Matcha-dark-azul`).
#[cfg(target_os = "android")]
fn is_dark_name(name: &str) -> bool {
    name.to_ascii_lowercase().contains("dark")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The portal answers with one of three strings; only the two preferences are a choice.
    #[test]
    #[cfg(target_os = "linux")]
    fn the_portal_answer_reads_as_a_choice_or_as_none() {
        assert_eq!(parse("prefer-dark"), Some(true));
        assert_eq!(parse("prefer-light"), Some(false));
        assert_eq!(parse("default"), None);
        assert_eq!(parse(""), None);
    }

    /// On macOS and Windows the caller already has winit's answer, so this adds nothing.
    #[test]
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn other_platforms_answer_through_winit() {
        assert_eq!(prefers_dark(), None);
    }

    /// XSETTINGS records, as XFCE's settings manager serializes them.
    #[test]
    #[cfg(target_os = "android")]
    fn xsettings_records_read_like_the_specification() {
        fn setting(ty: u8, name: &str, value: &[u8]) -> Vec<u8> {
            let mut out = vec![ty, 0, name.len() as u8, 0];
            out.extend_from_slice(name.as_bytes());
            out.resize(4 + name.len().div_ceil(4) * 4, 0);
            out.extend_from_slice(&0u32.to_le_bytes()); // serial
            out.extend_from_slice(value);
            out
        }
        let mut blob = b"l\0\0\0\0\0\0\0".to_vec();
        blob.extend_from_slice(&2u32.to_le_bytes());
        blob.extend_from_slice(&setting(0, "Xft/DPI", &(96 * 1024i32).to_le_bytes()));
        blob.extend_from_slice(&setting(1, "Net/ThemeName", &{
            let mut v = (12u32).to_le_bytes().to_vec();
            v.extend_from_slice(b"Adwaita-dark");
            v
        }));
        assert_eq!(xsettings_value(&blob, "Net/ThemeName"), Some(XValue::Text("Adwaita-dark".into())));
        assert_eq!(xsettings_value(&blob, "Xft/DPI"), Some(XValue::Integer(96 * 1024)));
        assert_eq!(xsettings_value(&blob, "Gtk/ApplicationPreferDarkTheme"), None);
    }

    /// The XFCE settings file and GTK's INI say the same thing in their own formats.
    #[test]
    #[cfg(target_os = "android")]
    fn settings_files_name_the_theme() {
        let xml = r#"<property name="Net" type="empty">
    <property name="ThemeName" type="string" value="Adwaita-dark"/>
    <property name="IconThemeName" type="empty"/>
  </property>"#;
        assert_eq!(xml_value(xml, "ThemeName"), Some("Adwaita-dark"));
        assert_eq!(xml_value(xml, "IconThemeName"), None);
        assert!(is_dark_name("Greybird-dark"));
        assert!(!is_dark_name("Adwaita"));
        let ini = "[Settings]\ngtk-theme-name=Adwaita-dark\n";
        assert_eq!(ini_value(ini, "Settings", "gtk-theme-name"), Some("Adwaita-dark"));
        assert_eq!(ini_value(ini, "Other", "gtk-theme-name"), None);
    }

    /// GTK's prefer-dark switch wins over the theme name when the two disagree.
    #[test]
    #[cfg(target_os = "android")]
    fn the_prefer_dark_switch_wins_over_the_name() {
        assert_eq!(prefer_dark_answer(Some("Adwaita"), Some(1)), Some(true));
        assert_eq!(prefer_dark_answer(Some("Adwaita-dark"), Some(0)), Some(true));
        assert_eq!(prefer_dark_answer(Some("Adwaita"), Some(0)), Some(false));
        assert_eq!(prefer_dark_answer(None, None), None);
    }

    /// A watcher that was never started is inert rather than a panic.
    #[test]
    fn a_watcher_without_a_thread_is_inert() {
        assert_eq!(DesktopTheme::default().take_change(), None);
    }

    /// A new app doesn't watch the desktop (the app binary starts the watcher), so tests and
    /// headless screenshots never depend on the host's theme or spawn D-Bus pollers.
    #[test]
    fn a_new_app_does_not_watch_the_desktop() {
        let app = crate::WordApp::new(wordcraft_engine::Session::new(wordcraft_doc::Document::new()), Default::default());
        assert!(!app.desktop_theme.watching());
    }
}
