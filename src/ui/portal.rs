//! The file chooser for other applications: an `org.freedesktop.impl.portal.FileChooser`
//! backend on the session bus. xdg-desktop-portal hands it the open and save dialogs of
//! applications that ask the portal (Flatpaks, Firefox, Qt and GTK applications set to use
//! it) when `portals.conf` names failBrauwser for that interface; `install.sh` sets that up.

use super::app::AppCtx;
use gtk::{gio, glib};
use std::cell::RefCell;
use std::rc::{Rc, Weak};

pub const BUS_NAME: &str = "org.freedesktop.impl.portal.desktop.failbrauwser";
const OBJECT: &str = "/org/freedesktop/portal/desktop";

const XML: &str = r#"<node>
  <interface name="org.freedesktop.impl.portal.FileChooser">
    <method name="OpenFile">
      <arg type="o" name="handle" direction="in"/><arg type="s" name="app_id" direction="in"/>
      <arg type="s" name="parent_window" direction="in"/><arg type="s" name="title" direction="in"/>
      <arg type="a{sv}" name="options" direction="in"/>
      <arg type="u" name="response" direction="out"/><arg type="a{sv}" name="results" direction="out"/>
    </method>
    <method name="SaveFile">
      <arg type="o" name="handle" direction="in"/><arg type="s" name="app_id" direction="in"/>
      <arg type="s" name="parent_window" direction="in"/><arg type="s" name="title" direction="in"/>
      <arg type="a{sv}" name="options" direction="in"/>
      <arg type="u" name="response" direction="out"/><arg type="a{sv}" name="results" direction="out"/>
    </method>
    <method name="SaveFiles">
      <arg type="o" name="handle" direction="in"/><arg type="s" name="app_id" direction="in"/>
      <arg type="s" name="parent_window" direction="in"/><arg type="s" name="title" direction="in"/>
      <arg type="a{sv}" name="options" direction="in"/>
      <arg type="u" name="response" direction="out"/><arg type="a{sv}" name="results" direction="out"/>
    </method>
  </interface>
  <interface name="org.freedesktop.impl.portal.Request">
    <method name="Close"/>
  </interface>
</node>"#;

thread_local! {
    /// The running application, once its startup is done (main thread).
    static APP: RefCell<Option<Weak<AppCtx>>> = const { RefCell::new(None) };
    /// Calls (dialogs, folders to show) that came before that.
    static WAITING: RefCell<Vec<Box<dyn FnOnce(Option<Rc<AppCtx>>)>>> = const { RefCell::new(Vec::new()) };
}

/// A dialog xdg-desktop-portal asks for.
struct Call {
    conn: gio::DBusConnection,
    method: String,
    params: glib::Variant,
    invocation: Invocation,
}

/// GDBus lets an invocation be answered from any thread; the bindings do not say so.
struct Invocation(gio::DBusMethodInvocation);
// SAFETY: a GDBusMethodInvocation does not change after it is made, and answering it only
// sends a message on its connection, which GDBus allows from any thread.
unsafe impl Send for Invocation {}

/// Takes the file chooser's bus name before GTK starts, and answers on it from a thread of
/// its own (once per process).
///
/// With no GNOME or Xfce session manager, GtkApplication's startup waits for
/// xdg-desktop-portal (its Inhibit interface), starting it if it does not run, and
/// xdg-desktop-portal waits for each of its backends to answer before it takes its name.
/// Answered from the main thread, failBrauwser waited on itself: after login both waited
/// out D-Bus's 25 second timeout, and with them every app that asked the portal anything,
/// as Qt apps do when they start.
pub fn start() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let spawned = std::thread::Builder::new().name("portal".into()).spawn(move || {
            let ctx = glib::MainContext::new();
            let _ = ctx.with_thread_default(|| {
                let lost = tx.clone();
                let _id = gio::bus_own_name(
                    gio::BusType::Session,
                    BUS_NAME,
                    gio::BusNameOwnerFlags::NONE,
                    |conn, _| register(&conn),
                    move |_, _| {
                        let _ = tx.send(());
                    },
                    move |_, _| {
                        let _ = lost.send(());
                    },
                );
                // "Show in folder" of other applications, answered the same way.
                let _fm = gio::bus_own_name(
                    gio::BusType::Session,
                    super::filemanager::BUS_NAME,
                    gio::BusNameOwnerFlags::NONE,
                    |conn, _| super::filemanager::register(&conn),
                    |_, _| {},
                    |_, _| {},
                );
                glib::MainLoop::new(Some(&ctx), false).run();
            });
        });
        // The name is ours (or we queue behind another failBrauwser) before GTK starts.
        if spawned.is_ok() {
            let _ = rx.recv_timeout(std::time::Duration::from_secs(2));
        }
    });
}

/// Hands the dialogs to the application once its startup is done (main thread).
pub fn serve(app: &Rc<AppCtx>) {
    APP.with(|a| *a.borrow_mut() = Some(Rc::downgrade(app)));
    for call in WAITING.with(|w| std::mem::take(&mut *w.borrow_mut())) {
        call(Some(app.clone()));
    }
}

/// Runs `f` with the application once its startup is done (`None` once it has gone; main
/// thread).
pub(super) fn with_app(f: impl FnOnce(Option<Rc<AppCtx>>) + 'static) {
    match APP.with(|a| a.borrow().as_ref().map(Weak::upgrade)) {
        None => WAITING.with(|w| w.borrow_mut().push(Box::new(f))),
        Some(app) => f(app),
    }
}

fn interface(name: &str) -> Option<gio::DBusInterfaceInfo> {
    gio::DBusNodeInfo::for_xml(XML).ok()?.lookup_interface(name)
}

/// On the portal thread: the dialogs themselves are made on the main thread.
fn register(conn: &gio::DBusConnection) {
    let Some(iface) = interface("org.freedesktop.impl.portal.FileChooser") else { return };
    let _ = conn
        .register_object(OBJECT, &iface)
        .method_call(|conn, _, _, _, method, params, invocation| {
            let call = Call { conn, method: method.to_string(), params, invocation: Invocation(invocation) };
            glib::MainContext::default().invoke(move || answer_or_wait(call));
        })
        .build();
}

fn answer_or_wait(call: Call) {
    with_app(move |app| answer(app, call));
}

fn answer(app: Option<Rc<AppCtx>>, call: Call) {
    let Call { conn, method, params, invocation: Invocation(invocation) } = call;
    let Some(app) = app else {
        invocation.return_value(Some(&failbrauwser::chooser::reply(None)));
        return;
    };
    let method = method.as_str();
    let request_iface = interface("org.freedesktop.impl.portal.Request");
    let conn2 = conn;
    let handle: String = params.child_value(0).str().unwrap_or_default().to_string();
    let title: String = params.child_value(3).get().unwrap_or_default();
    let request = failbrauwser::chooser::parse(method, &title, &params.child_value(4));
    // The application may close the dialog itself, through the request's object.
    let registration: Rc<RefCell<Option<gio::RegistrationId>>> = Rc::new(RefCell::new(None));
    let r2 = registration.clone();
    let conn3 = conn2.clone();
    let chooser = super::chooser::open(
        &app,
        request,
        Box::new(move |answer| {
            if let Some(id) = r2.borrow_mut().take() {
                let _ = conn3.unregister_object(id);
            }
            let picked = answer.as_ref().map(|(paths, filter)| (paths.as_slice(), filter.as_ref()));
            invocation.return_value(Some(&failbrauwser::chooser::reply(picked)));
        }),
    );
    if let (Some(req), false) = (&request_iface, handle.is_empty()) {
        let weak = Rc::downgrade(&chooser);
        if let Ok(id) = conn2
            .register_object(&handle, req)
            .method_call(move |_, _, _, _, _, _, inv| {
                if let Some(c) = weak.upgrade() {
                    c.cancel();
                }
                inv.return_value(None);
            })
            .build()
        {
            *registration.borrow_mut() = Some(id);
        }
    }
}
