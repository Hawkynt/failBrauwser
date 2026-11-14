//! Follows the desktop's light or dark mode while running (see [`failbrauwser::appearance`]):
//! the portal's colour scheme and GNOME's `color-scheme` and `gtk-theme` are watched, and
//! every window restyles the moment they change.

use failbrauwser::appearance::{self, Look, Scheme};
use gtk::prelude::*;
use gtk::{gio, glib};
use std::cell::RefCell;

const INTERFACE: &str = "org.gnome.desktop.interface";

struct State {
    /// The colour scheme last heard of.
    scheme: Scheme,
    /// Changes heard of since the start (a late first answer must not undo them).
    changes: u32,
    /// GTK's theme before anything was changed, for desktops without GNOME's settings.
    startup_theme: String,
    gsettings: Option<gio::Settings>,
    _portal: Option<gio::SignalSubscription>,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

/// GNOME's interface settings, when installed.
fn interface_settings() -> Option<gio::Settings> {
    let schema = gio::SettingsSchemaSource::default()?.lookup(INTERFACE, true)?;
    schema.has_key("color-scheme").then(|| gio::Settings::new(INTERFACE))
}

/// Starts following the desktop (once per process).
pub fn follow() {
    if STATE.with(|s| s.borrow().is_some()) {
        return;
    }
    let Some(settings) = gtk::Settings::default() else { return };
    let startup_theme = settings.gtk_theme_name().map(|s| s.to_string()).unwrap_or_else(|| "Adwaita".into());
    let gsettings = interface_settings();
    // Windows spells its preference in the registry (HKCU\...\Personalize); no GNOME
    // settings, no portal — one quiet poll follows the Settings app switching modes.
    #[cfg(windows)]
    let scheme = failbrauwser::win::reg::apps_use_light_theme().map_or(Scheme::Default, |light| if light { Scheme::Light } else { Scheme::Dark });
    #[cfg(not(windows))]
    let scheme = gsettings.as_ref().map_or(Scheme::Default, |g| Scheme::from_gsettings(&g.string("color-scheme")));
    if let Some(g) = &gsettings {
        g.connect_changed(Some("color-scheme"), |g, _| set_scheme(Scheme::from_gsettings(&g.string("color-scheme"))));
        g.connect_changed(Some("gtk-theme"), |_, _| apply());
    }
    // The portal tells sandboxed and non-GNOME desktops' changes too.
    #[cfg(windows)]
    let portal: Option<gio::SignalSubscription> = None;
    #[cfg(not(windows))]
    let portal = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE).ok().map(|conn| {
        conn.subscribe_to_signal(
            Some("org.freedesktop.portal.Desktop"),
            Some("org.freedesktop.portal.Settings"),
            Some("SettingChanged"),
            Some("/org/freedesktop/portal/desktop"),
            None,
            gio::DBusSignalFlags::NONE,
            |signal| {
                let p = signal.parameters;
                let (ns, key) = (p.child_value(0).str().unwrap_or_default().to_string(), p.child_value(1).str().unwrap_or_default().to_string());
                if ns == "org.freedesktop.appearance" && key == "color-scheme" {
                    if let Some(v) = p.child_value(2).as_variant().and_then(|v| v.get::<u32>()) {
                        set_scheme(Scheme::from_portal(v));
                    }
                }
            },
        )
    });
    STATE.with(|s| *s.borrow_mut() = Some(State { scheme, changes: 0, startup_theme, gsettings, _portal: portal }));
    apply();
    // Windows has no signal for the theme keys: read the one value on a slow timer.
    #[cfg(windows)]
    {
        let mut last = scheme;
        glib::timeout_add_local(std::time::Duration::from_millis(2_000), move || {
            if let Some(now) = failbrauwser::win::reg::apps_use_light_theme().map(|light| if light { Scheme::Light } else { Scheme::Dark }) {
                if now != last {
                    last = now;
                    set_scheme(now);
                }
            }
            glib::ControlFlow::Continue
        });
    }
    // The portal knows the scheme where GNOME's settings do not.
    #[cfg(not(windows))]
    glib::spawn_future_local(async {
        let Ok(conn) = gio::bus_get_future(gio::BusType::Session).await else { return };
        let args = ("org.freedesktop.appearance", "color-scheme").to_variant();
        let reply = conn
            .call_future(Some("org.freedesktop.portal.Desktop"), "/org/freedesktop/portal/desktop", "org.freedesktop.portal.Settings", "ReadOne", Some(&args), None, gio::DBusCallFlags::NONE, 2000)
            .await;
        // The value comes wrapped once, or twice by older portals.
        let unwrap = |v: glib::Variant| if v.is_type(glib::VariantTy::VARIANT) { v.as_variant() } else { Some(v) };
        if let Some(v) = reply.ok().map(|r| r.child_value(0)).and_then(unwrap).and_then(unwrap).and_then(|v| v.get::<u32>()) {
            // Only if nothing changed while the portal was asked.
            if STATE.with(|s| s.borrow().as_ref().is_some_and(|st| st.changes == 0)) {
                set_scheme(Scheme::from_portal(v));
            }
        }
    });
}

/// A new colour scheme (from the desktop, or the self-test).
pub fn set_scheme(scheme: Scheme) {
    let changed = STATE.with(|s| {
        s.borrow_mut().as_mut().is_some_and(|st| {
            st.changes += 1;
            std::mem::replace(&mut st.scheme, scheme) != scheme
        })
    });
    if changed {
        apply();
    }
}

/// The theme the desktop sets.
fn desktop_theme(st: &State) -> String {
    st.gsettings.as_ref().map(|g| g.string("gtk-theme").to_string()).filter(|t| !t.is_empty()).unwrap_or_else(|| st.startup_theme.clone())
}

fn apply() {
    let Some(settings) = gtk::Settings::default() else { return };
    let look = STATE.with(|s| {
        let s = s.borrow();
        let st = s.as_ref()?;
        (st.scheme != Scheme::Default).then(|| appearance::look_for(&desktop_theme(st), st.scheme, appearance::theme_exists, appearance::theme_has_dark))
    });
    match look {
        Some(Look { theme, prefer_dark }) => {
            if settings.gtk_theme_name().as_deref() != Some(theme.as_str()) {
                settings.set_gtk_theme_name(Some(&theme));
            }
            settings.set_gtk_application_prefer_dark_theme(prefer_dark);
        }
        // No preference: what GTK takes from the desktop and its settings files.
        None => {
            settings.reset_property("gtk-theme-name");
            settings.reset_property("gtk-application-prefer-dark-theme");
        }
    }
}

/// Whether a window is drawn dark now: its background as the theme paints it (tests).
pub fn window_is_dark(win: &gtk::Window) -> Option<bool> {
    let bg = win.style_context().lookup_color("theme_bg_color")?;
    Some(0.2126 * bg.red() + 0.7152 * bg.green() + 0.0722 * bg.blue() < 0.5)
}
