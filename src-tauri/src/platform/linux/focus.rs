//! Which window is active, on KDE. Wayland tells no application about another
//! application's windows, but KWin runs scripts and a script can call D-Bus:
//! the one loaded here tells the meter each time the active window changes,
//! for "Hide when AION2 isn't focused". Other desktops are not covered.
//!
//! On Wayland it also keeps the meter's windows where they were left (#76):
//! no application may place its own windows there, but a KWin script may.
//! The script reports where each of the meter's windows was moved or resized
//! to, and puts each back there when it appears again.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

const SERVICE: &str = "app.a2tools.DpsMeter";
const PATH: &str = "/Focus";
/// The script's name in KWin, so a script left by an earlier run is replaced.
const PLUGIN: &str = "a2tools-dps-meter-focus";

/// Values are passed as strings: KWin hands JavaScript numbers to D-Bus as
/// doubles, which an integer argument would refuse. KWin 6 has
/// `windowActivated` and `windowAdded`, KWin 5 `clientActivated` and
/// `clientAdded`. `__PID__` is the meter's process id: only its windows are
/// placed.
const SCRIPT: &str = r#"// A2Tools DPS Meter: tells the meter which window is active, so it can hide
// while the game is not in front, and keeps the meter's own windows where
// they were left. Loaded by the meter at each start.
var SERVICE = "app.a2tools.DpsMeter", PATH = "/Focus", IFACE = "app.a2tools.DpsMeter.Focus";
var MY_PID = __PID__;
function report(w) {
    callDBus(SERVICE, PATH, IFACE, "Active",
             w ? String(w.pid) : "0", w ? String(w.resourceClass) : "", w ? String(w.caption) : "");
}
function mine(w) {
    return !!w && Number(w.pid) === MY_PID;
}
// A plugin's widget (an alert's icon), titled "<plugin> · <name>".
function widget(w) {
    return mine(w) && String(w.caption).indexOf(" · ") >= 0;
}
// The last window not ours to be active, and our widgets that just appeared:
// Wayland activates a new window, so each icon an alert opened took the
// focus from the game (a tester, mid-PvP, 2026-10-17). An icon activated
// because it appeared hands the focus straight back; one clicked keeps it.
var lastOther = null;
var fresh = [];
// The active window and the one before it. KWin activates a new window
// before it announces it (windowActivated, then windowAdded: seen on KDE
// Plasma 6, 2026-10-19), so an icon is already active when follow() meets it.
var current = null, previous = null;
function activate(w) {
    if (workspace.windowActivated) workspace.activeWindow = w;
    else workspace.activeClient = w;
}
function onActivated(w) {
    if (w !== current) {
        previous = current;
        current = w;
    }
    if (w && !mine(w)) lastOther = w;
    var i = fresh.indexOf(w);
    if (i >= 0) {
        fresh.splice(i, 1);
        if (lastOther) {
            activate(lastOther);
            return;
        }
    }
    report(w);
}
function remember(w) {
    var g = w.frameGeometry;
    callDBus(SERVICE, PATH, IFACE, "Geometry", String(w.caption), String(Math.round(g.x)), String(Math.round(g.y)),
             String(Math.round(g.width)), String(Math.round(g.height)));
}
function onScreen(r) {
    var area = workspace.virtualScreenGeometry;
    if (!area) return true;
    // Enough of its top edge on screen to be seen and dragged back.
    return r.x + 48 <= area.x + area.width && r.x + r.width >= area.x + 48
        && r.y >= area.y - 8 && r.y + 32 <= area.y + area.height;
}
// How long a window is held at its place: the meter sizes its windows to
// their content just after they appear, and KWin keeps a resized window's
// centre where it was, so the overlay, 300 px tall at first, came down
// 120 px once it shrank to its rows (a CachyOS player, 2026-10-19).
var HOLD_MS = 4000;
function anchor(caption, then) {
    callDBus(SERVICE, PATH, IFACE, "Anchor", String(caption), function (answer) {
        then(String(answer || "middle"));
    });
}
function place(w) {
    callDBus(SERVICE, PATH, IFACE, "Placement", String(w.caption), function (answer) {
        var p = String(answer).split(",").map(Number);
        if (p.length !== 4 || p.some(isNaN)) return;
        var g = w.frameGeometry;
        // A width or height of 0: the window keeps its own size.
        var r = { x: p[0], y: p[1], width: p[2] > 0 ? p[2] : g.width, height: p[3] > 0 ? p[3] : g.height };
        if (!onScreen(r)) return;
        w.frameGeometry = r;
        // Moved by a resize while held: back to its place, at its new size.
        // Never against the player's own move or resize.
        var until = Date.now() + HOLD_MS;
        var held = false;
        var userMove = false;
        var seq = 0;
        // The meter's main overlay keeps its top/middle/bottom anchor while
        // its own auto-resize changes its height.
        var main = String(w.caption) === "A2Tools DPS Meter";
        var base = { x: r.x, top: r.y, bottom: r.y + r.height };
        var start = w.interactiveMoveResizeStarted || w.clientStartUserMovedResized;
        if (start) {
            start.connect(function () {
                userMove = true;
                until = 0;
            });
        }
        var done = w.interactiveMoveResizeFinished || w.clientFinishUserMovedResized;
        if (done) {
            done.connect(function () {
                userMove = false;
                var now = w.frameGeometry;
                base.x = Math.round(now.x);
                base.top = Math.round(now.y);
                base.bottom = base.top + Math.round(now.height);
            });
        }
        w.frameGeometryChanged.connect(function () {
            if (held || userMove || w.move || w.resize) return;
            var now = w.frameGeometry;
            if (!main) {
                if (Date.now() > until) return;
                if (Math.round(now.x) === r.x && Math.round(now.y) === r.y) return;
                held = true;
                w.frameGeometry = { x: r.x, y: r.y, width: now.width, height: now.height };
                held = false;
                return;
            }
            var ticket = ++seq;
            anchor(w.caption, function (mode) {
                if (ticket !== seq || held || userMove || w.move || w.resize) return;
                var live = w.frameGeometry;
                var x = Math.round(live.x), y = Math.round(live.y), h = Math.round(live.height);
                var wantX = base.x, wantY = base.top;
                if (mode === "bottom") wantY = base.bottom - h;
                else if (mode === "middle") {
                    if (Date.now() > until) return;
                    wantX = r.x;
                    wantY = r.y;
                }
                // Scale rounding can report +/- 1 px between events.
                if (Math.abs(x - wantX) <= 1 && Math.abs(y - wantY) <= 1) return;
                held = true;
                w.frameGeometry = { x: wantX, y: wantY, width: live.width, height: live.height };
                held = false;
            });
        });
    });
}
function follow(w) {
    if (!mine(w)) return;
    // An icon already active as it is added: KWin activated it first. The
    // focus goes back to the window active before it. (The KWin rule for
    // icons keeps them from taking it at all; this is for when the rule is
    // missing, as KWin can lose it.)
    var aw = workspace.activeWindow || workspace.activeClient;
    if (widget(w) && aw === w && previous && previous !== w && !previous.deleted) activate(previous);
    else if (widget(w)) {
        fresh.push(w);
        if (fresh.length > 32) fresh.shift();
    } else {
        // Another of the meter's windows the meter showed on its own (back
        // with the game after hiding, a plugin showing itself): KWin takes it
        // for a new window and activates it, so it took the focus from the
        // game (2.0.76, now that hidden windows are unmapped). It hands the
        // focus back, as an icon does; one the player opened keeps it.
        callDBus(SERVICE, PATH, IFACE, "Quiet", String(w.caption), function (answer) {
            if (String(answer) !== "true") return;
            var now = workspace.activeWindow || workspace.activeClient;
            if (now === w && lastOther && !lastOther.deleted) activate(lastOther);
            else {
                fresh.push(w);
                if (fresh.length > 32) fresh.shift();
            }
        });
    }
    var done = w.interactiveMoveResizeFinished || w.clientFinishUserMovedResized;
    if (done) done.connect(function () { remember(w); });
    place(w);
}
// A window of the meter going away (hidden: on KDE the meter unmaps the
// windows it hides) leaves its place behind, so it comes back there.
function gone(w) {
    if (mine(w) && onScreen(w.frameGeometry)) remember(w);
}
if (workspace.windowRemoved) workspace.windowRemoved.connect(gone);
else if (workspace.clientRemoved) workspace.clientRemoved.connect(gone);
if (workspace.windowActivated) {
    workspace.windowActivated.connect(onActivated);
    report(workspace.activeWindow);
    workspace.windowAdded.connect(follow);
    workspace.windowList().forEach(follow);
} else {
    workspace.clientActivated.connect(onActivated);
    report(workspace.activeClient);
    workspace.clientAdded.connect(follow);
    workspace.clientList().forEach(follow);
}
"#;

/// A window's place and size, as KWin gives them: x, y, width, height.
pub type Rect = (i32, i32, i32, i32);

/// Where the meter keeps the places KWin reports, by window title.
pub struct Places {
    pub save: Box<dyn Fn(&str, Rect) + Send + Sync>,
    pub load: Box<dyn Fn(&str) -> Option<Rect> + Send + Sync>,
}

static PLACES: OnceLock<Places> = OnceLock::new();

/// The meter runs on X11 (`GDK_BACKEND=x11`, XWayland) in a Wayland session.
/// It places and remembers its own windows then, but KWin keeps a new X11
/// window inside the X11 work area: one rectangle for every monitor, its
/// bottom the top of the lowest panel on any of them. So a widget left below
/// that line on another monitor came back above it at each start (Ping on a
/// 1440 px monitor, a panel on a 1080 px one: back at the 1232 line). KWin
/// places only widgets then, where the meter left them (`load`).
static X11_IN_WAYLAND: AtomicBool = AtomicBool::new(false);

/// The script is running, so KWin puts each of the meter's windows back where
/// it was each time it appears (`place` in the script).
static PLACING: AtomicBool = AtomicBool::new(false);
/// 0 top, 1 middle (default), 2 bottom.
static MAIN_ANCHOR: AtomicU8 = AtomicU8::new(1);

/// Keep the meter's windows where they were left, through KWin (#76). Only
/// in a Wayland session: under X11 the meter places its own windows, as on
/// Windows (in a Wayland session too, but for its widgets: X11_IN_WAYLAND).
pub fn keep_places(places: Places) {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        let x11 = std::env::var("GDK_BACKEND").is_ok_and(|b| b.starts_with("x11"));
        X11_IN_WAYLAND.store(x11, Ordering::Relaxed);
        let _ = PLACES.set(places);
    }
}

/// Whether the meter places and remembers its own windows, KWin placing only
/// its widgets (see X11_IN_WAYLAND).
pub fn places_own_windows() -> bool {
    X11_IN_WAYLAND.load(Ordering::Relaxed)
}

/// Whether KWin puts the meter's windows back where they were whenever they
/// appear: on native Wayland in KDE, with the script running. A window can
/// then be hidden for good and shown again at its place.
pub fn kwin_places_windows() -> bool {
    PLACING.load(Ordering::Relaxed) && PLACES.get().is_some() && !X11_IN_WAYLAND.load(Ordering::Relaxed)
}

pub fn set_main_anchor(anchor: &str) {
    let v = match anchor.trim().to_ascii_lowercase().as_str() {
        "top" => 0,
        "bottom" => 2,
        _ => 1,
    };
    MAIN_ANCHOR.store(v, Ordering::Relaxed);
}

/// 0: not known (no report yet, or not KDE). 1: the game or one of the
/// meter's own windows is active. 2: something else is.
static ACTIVE: AtomicU8 = AtomicU8::new(0);

/// The game's window has been reported active at least once. Until then the
/// reports are not trusted: the setting is on by default, and a game window
/// this does not recognise must not leave the meter hidden for good.
static SEEN_GAME: AtomicBool = AtomicBool::new(false);

/// The meter's windows it is showing on its own, by title, and since when:
/// they hand the focus back as they appear (`Focus::quiet`).
static QUIET: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>> =
    std::sync::LazyLock::new(Default::default);
/// How long a quiet show waits for its window to appear.
const QUIET_FOR: std::time::Duration = std::time::Duration::from_secs(3);

/// The meter is showing the window titled `title` on its own, not because the
/// player asked: it is not to take the focus (`Focus::quiet`).
pub fn note_quiet_show(title: &str) {
    if PLACING.load(Ordering::Relaxed) {
        QUIET.lock().unwrap_or_else(|p| p.into_inner()).insert(title.to_string(), std::time::Instant::now());
    }
}

/// The connection that serves `Focus`; dropped, the service would go away.
static CONNECTION: OnceLock<zbus::blocking::Connection> = OnceLock::new();

/// Told when the game or the meter gains or loses the front.
static ON_CHANGE: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

/// Whether a screenshot tool (Spectacle) is the active window, and who is
/// told when that changes (`watch_screenshot_tool`).
static SCREENSHOT: AtomicBool = AtomicBool::new(false);
static ON_SCREENSHOT: OnceLock<Box<dyn Fn(bool) + Send + Sync>> = OnceLock::new();

/// Be told when a screenshot tool becomes, or stops being, the active window.
pub fn watch_screenshot_tool(on_change: impl Fn(bool) + Send + Sync + 'static) {
    let _ = ON_SCREENSHOT.set(Box::new(on_change));
}

/// KDE's Spectacle, by its window class (Plasma 6: `org.kde.spectacle`,
/// Plasma 5: `spectacle`). Its area selection is drawn under the meter's
/// windows (the overlay layer, see `kwin_rules`), so they took the drag
/// (Zoe, CachyOS, 2026-10-20).
fn is_screenshot_tool(class: &str) -> bool {
    let class = class.trim().to_ascii_lowercase();
    class == "org.kde.spectacle" || class == "spectacle"
}

/// Whether the game (or the meter) is the active window, if KWin has said
/// and the game has been recognised.
pub fn game_or_meter_active() -> Option<bool> {
    if !SEEN_GAME.load(Ordering::Relaxed) {
        return None;
    }
    match ACTIVE.load(Ordering::Relaxed) {
        0 => None,
        1 => Some(true),
        _ => Some(false),
    }
}

struct Focus;

#[zbus::interface(name = "app.a2tools.DpsMeter.Focus")]
impl Focus {
    fn active(&self, pid: String, class: String, caption: String) {
        let shot = is_screenshot_tool(&class);
        if SCREENSHOT.swap(shot, Ordering::Relaxed) != shot {
            tracing::info!("KWin: screenshot tool {}", if shot { "in front: the meter lets clicks through" } else { "gone" });
            if let Some(on_screenshot) = ON_SCREENSHOT.get() {
                on_screenshot(shot);
            }
        }
        let who = classify(pid.parse().unwrap_or(0), &class, &caption);
        if who == Who::Game && !SEEN_GAME.swap(true, Ordering::Relaxed) {
            tracing::info!("KWin: the game's window is active (class {class:?})");
        }
        let was = ACTIVE.swap(if who == Who::Other { 2 } else { 1 }, Ordering::Relaxed);
        if was != ACTIVE.load(Ordering::Relaxed) && SEEN_GAME.load(Ordering::Relaxed) {
            if let Some(on_change) = ON_CHANGE.get() {
                on_change();
            }
        }
    }

    /// One of the meter's windows was moved or resized by the player.
    fn geometry(&self, caption: String, x: String, y: String, width: String, height: String) {
        // On X11 the meter saves its own windows' places as they move.
        if places_own_windows() {
            return;
        }
        let Some(places) = PLACES.get() else { return };
        let n = |v: &str| v.trim().parse::<i32>().ok();
        if let (Some(x), Some(y), Some(w), Some(h)) = (n(&x), n(&y), n(&width), n(&height))
            && w > 0
            && h > 0
        {
            (places.save)(&caption, (x, y, w, h));
        }
    }

    /// Whether the window titled `caption`, just appeared, was shown by the
    /// meter on its own (`note_quiet_show`), so it gives the focus back.
    fn quiet(&self, caption: String) -> bool {
        let mut quiet = QUIET.lock().unwrap_or_else(|p| p.into_inner());
        quiet.retain(|_, at| at.elapsed() < QUIET_FOR);
        quiet.remove(&caption).is_some()
    }

    /// Where one of the meter's windows belongs, `"x,y,width,height"`, or ""
    /// to leave it where KWin put it.
    fn placement(&self, caption: String) -> String {
        PLACES
            .get()
            .and_then(|places| (places.load)(&caption))
            .map(|(x, y, w, h)| format!("{x},{y},{w},{h}"))
            .unwrap_or_default()
    }

    /// The configured growth anchor for the main overlay.
    fn anchor(&self, caption: String) -> String {
        if caption.trim() != "A2Tools DPS Meter" {
            return "middle".into();
        }
        match MAIN_ANCHOR.load(Ordering::Relaxed) {
            0 => "top".into(),
            2 => "bottom".into(),
            _ => "middle".into(),
        }
    }
}

#[derive(Debug, PartialEq)]
enum Who {
    Game,
    Meter,
    Other,
}

/// The game by title or class, as on Windows, or by its process; the meter by
/// its own process id, so opening Settings does not hide the overlay.
fn classify(pid: u32, class: &str, caption: &str) -> Who {
    if caption.starts_with("AION2") || class.eq_ignore_ascii_case("aion2.exe") {
        return Who::Game;
    }
    if pid == 0 {
        return Who::Other;
    }
    if pid == std::process::id() {
        return Who::Meter;
    }
    let dir = std::path::PathBuf::from(format!("/proc/{pid}"));
    let comm = std::fs::read_to_string(dir.join("comm")).unwrap_or_default();
    let cmdline = std::fs::read(dir.join("cmdline")).unwrap_or_default();
    if crate::platform::procfs::is_aion2_process(&comm, &cmdline) { Who::Game } else { Who::Other }
}

/// Serve `Focus` on the session bus and load the script into KWin. Does
/// nothing outside KDE: there the game is taken as always focused.
pub fn watch(on_change: impl Fn() + Send + Sync + 'static) -> Result<(), String> {
    if !super::process::is_kde() || CONNECTION.get().is_some() {
        return Ok(());
    }
    let connection = zbus::blocking::connection::Builder::session()
        .and_then(|b| b.name(SERVICE))
        .and_then(|b| b.serve_at(PATH, Focus))
        .and_then(|b| b.build())
        .map_err(|e| format!("D-Bus service: {e}"))?;

    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let path = dir.join("a2tools-dps-meter-focus.js");
    let script = SCRIPT.replace("__PID__", &std::process::id().to_string());
    std::fs::write(&path, script).map_err(|e| format!("writing the script: {e}"))?;

    let kwin = zbus::blocking::Proxy::new(&connection, "org.kde.KWin", "/Scripting", "org.kde.kwin.Scripting")
        .map_err(|e| format!("KWin scripting: {e}"))?;
    let _ = kwin.call_method("unloadScript", &(PLUGIN,));
    let id: i32 = kwin
        .call("loadScript", &(path.to_string_lossy().as_ref(), PLUGIN))
        .map_err(|e| format!("loading the script: {e}"))?;
    if id < 0 {
        return Err("KWin did not load the script".into());
    }
    kwin.call_method("start", &()).map_err(|e| format!("starting the script: {e}"))?;
    let _ = ON_CHANGE.set(Box::new(on_change));
    let _ = CONNECTION.set(connection);
    PLACING.store(true, Ordering::Relaxed);
    tracing::info!("KWin reports the active window (script {id})");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spectacle_is_known_by_its_class() {
        assert!(is_screenshot_tool("org.kde.spectacle"));
        assert!(is_screenshot_tool("spectacle"));
        assert!(!is_screenshot_tool("org.kde.konsole"));
        assert!(!is_screenshot_tool(""));
    }

    #[test]
    fn the_game_is_known_by_title_or_class_and_the_meter_by_itself() {
        assert_eq!(classify(0, "", "AION2"), Who::Game);
        assert_eq!(classify(0, "aion2.exe", ""), Who::Game);
        assert_eq!(classify(std::process::id(), "a2tools-dps-meter", "A2Tools DPS Meter"), Who::Meter);
        assert_eq!(classify(0, "firefox", "Mozilla Firefox"), Who::Other);
    }
}
