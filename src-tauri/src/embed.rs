//! Embed an external app (Opera) inside the Tauri window via Win32 SetParent.
//! Windows-only. On other OSes the commands return a friendly error.

use std::time::Duration;
use tauri::{AppHandle, Manager, Runtime, State};

/// Stored as raw isize so the state stays Send + Sync without depending on HWND traits.
/// `pending` keeps the last frontend rect so early resize calls (sent before
/// Opera's window exists) aren't lost — they're applied the moment we embed.
#[derive(Default)]
pub struct EmbedStateInner {
    pub window: Option<(isize, u32)>,
    /// Extra app spawned via `launch_embedded_app` and awaiting manual
    /// docking through `embed_launched_app`: (pid, display name).
    pub launched: Option<(u32, String)>,
    /// Last frontend rect: (x, y, w, h, scale, viewport_w, viewport_h).
    /// x/y/w/h are CSS px relative to the WebView viewport (= parent client
    /// area, since the WebView fills it). vw/vh let consumers clip to what is
    /// actually on screen.
    pub pending: Option<(i32, i32, i32, i32, f64, f64, f64)>,
    /// Raw parent (Tauri main window) address for ClientToScreen mapping.
    pub parent: Option<isize>,
    /// The app_id of the currently embedded extra app (if any).
    /// Used to forward inputs via MCP when enabled.
    pub embedded_app_id: Option<String>,
    /// The vehicle ID of the currently embedded app (for MCP input forwarding).
    pub embedded_vehicle_id: Option<i32>,
    /// MCP configuration for the currently launched app
    pub mcp_enabled: bool,
    pub mcp_host: String,
    pub mcp_port: u16,
}

/// Re-position the embedded child. Called from the stream loop too so the
/// child stays glued to its HTML section at capture fps while streaming.
#[cfg(windows)]
pub(crate) fn place_child_raw(child_raw: isize, x: i32, y: i32, w: i32, h: i32) {
    use windows::Win32::Foundation::HWND;
    use win::place;
    place(HWND(child_raw as *mut _), x, y, w, h);
}
pub struct EmbedState(pub std::sync::Mutex<EmbedStateInner>);

#[cfg(windows)]
mod win {
    use windows::Win32::Foundation::{BOOL, FALSE, HWND, LPARAM, TRUE};
    use windows::Win32::UI::WindowsAndMessaging::*;

    struct Search {
        pid: u32,
        found: HWND,
    }

    unsafe extern "system" fn enum_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let search = unsafe { &mut *(lparam.0 as *mut Search) };
        let mut wpid: u32 = 0;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut wpid)) };
        if wpid == search.pid && unsafe { IsWindowVisible(hwnd).as_bool() } {
            // Take the first visible top-level window owned by this PID.
            // Opera's helper processes are usually invisible, so this skips them.
            search.found = hwnd;
            return BOOL(0); // stop enumeration
        }
        BOOL(1) // continue
    }

    pub fn find_hwnd_for_pid(pid: u32) -> Option<HWND> {
        let mut search = Search {
            pid,
            found: HWND::default(),
        };
        unsafe {
            let _ = EnumWindows(
                Some(enum_cb),
                LPARAM(&mut search as *mut _ as isize),
            );
        }
        if search.found.is_invalid() {
            None
        } else {
            Some(search.found)
        }
    }

    /// Focus WITHOUT foreground, for background wheel delivery.
    ///
    /// Chromium drops wheel input that arrives with no focus context, but
    /// `SetForegroundWindow` would pop the whole app over the game (the bug
    /// we fixed with the focus-steal change). Instead: attach our input
    /// queue to the target thread and `SetFocus` the embedded window. The
    /// foreground window (BeamNG) never changes, so nothing visibly flickers.
    /// The guard detaches on drop. Best-effort: an empty guard when the
    /// target thread can't be reached.
    pub struct FocusGuard {
        cur: u32,
        target: u32,
        attached: bool,
    }

    impl FocusGuard {
        pub fn focus(hwnd: HWND) -> Self {
            use windows::Win32::System::Threading::{
                AttachThreadInput, GetCurrentThreadId,
            };
            use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
            use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
            let mut target = 0u32;
            unsafe { GetWindowThreadProcessId(hwnd, Some(&mut target)) };
            if target == 0 {
                return Self { cur: 0, target: 0, attached: false };
            }
            let cur = unsafe { GetCurrentThreadId() };
            if cur == target {
                unsafe {
                    let _ = SetFocus(hwnd);
                };
                return Self { cur, target, attached: false };
            }
            let attached =
                unsafe { AttachThreadInput(cur, target, TRUE).as_bool() };
            if attached {
                unsafe {
                    let _ = SetFocus(hwnd);
                };
            }
            Self { cur, target, attached }
        }
    }

    impl Drop for FocusGuard {
        fn drop(&mut self) {
            if self.attached {
                use windows::Win32::System::Threading::AttachThreadInput;
                unsafe {
                    let _ = AttachThreadInput(self.cur, self.target, FALSE);
                }
            }
        }
    }

    struct TopCollect {
        pid: u32,
        out: Vec<isize>,
    }

    unsafe extern "system" fn tops_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let c = unsafe { &mut *(lparam.0 as *mut TopCollect) };
        let mut p: u32 = 0;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut p)) };
        if p == c.pid {
            c.out.push(hwnd.0 as isize);
        }
        BOOL(1)
    }

    /// All current top-level windows owned by `pid` (embedded or stray).
    pub fn top_windows_for_pid(pid: u32) -> Vec<isize> {
        let mut c = TopCollect { pid, out: Vec::new() };
        unsafe {
            let _ = EnumWindows(Some(tops_cb), LPARAM(&mut c as *mut _ as isize));
        }
        c.out
    }

    pub fn strip_frame(hwnd: HWND) {
        unsafe {
            let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
            let style = style & !(WS_CAPTION.0 | WS_THICKFRAME.0 | WS_BORDER.0 | WS_DLGFRAME.0 | WS_OVERLAPPED.0);
            let style = style | WS_POPUP.0 | WS_CHILD.0 | WS_CLIPSIBLINGS.0 | WS_CLIPCHILDREN.0;
            SetWindowLongW(hwnd, GWL_STYLE, style as i32);
            // Also clear EX_LAYERED/TRANSPARENT bits that can make Chromium paint black.
            let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
            let ex = ex & !(WS_EX_LAYERED.0 | WS_EX_TRANSPARENT.0);
            SetWindowLongW(hwnd, GWL_EXSTYLE, ex as i32);
            let _ = SetWindowPos(
                hwnd,
                None,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED,
            );
        }
    }

    /// Position + force above the WebView sibling so it's actually visible.
    /// Tauri's WebView covers the whole client area, so a reparented child
    /// defaults to *behind* it unless we pass HWND_TOP.
    pub fn place(hwnd: HWND, x: i32, y: i32, w: i32, h: i32) {
        unsafe {
            let _ = MoveWindow(hwnd, x, y, w.max(50), h.max(50), true);
            let _ = SetWindowPos(hwnd, HWND_TOP, x, y, w.max(50), h.max(50), SWP_SHOWWINDOW);
            let _ = ShowWindow(hwnd, SW_SHOW);
        }
    }

    /// Deepest input-capable descendant of the embedded Opera top window.
    ///
    /// Chromium routes real input to `Chrome_RenderWidgetHostHWND` views, not
    /// the top-level frame. Clicks/keys posted to the top window get forwarded
    /// by the frame, but the wheel is dropped there while unfocused — it only
    /// lands when posted to the render host itself. Pages often own SEVERAL
    /// hosts (main frame + OOPIF ad/iframe hosts, some hidden), so first-found
    /// can hit an invisible one: prefer the deepest host whose rect contains
    /// the wheel point, else the largest host, else `top` (page loading).
    pub fn wheel_host(top: HWND, sx: i32, sy: i32) -> HWND {
        use windows::Win32::Foundation::RECT;
        use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;

        fn walk(raw: isize, depth: i32, out: &mut Vec<(isize, i32)>) {
            out.push((raw, depth));
            let mut next = Vec::new();
            unsafe {
                let _ = EnumChildWindows(
                    HWND(raw as *mut _),
                    Some(collect_cb),
                    LPARAM(&mut next as *mut _ as isize),
                );
            }
            for c in next {
                walk(c, depth + 1, out);
            }
        }

        let mut all = Vec::new();
        walk(top.0 as isize, 0, &mut all);
        let mut fallback = top;
        let mut fallback_area: i64 = -1;
        let mut best: Option<(isize, i32)> = None;
        for (raw, depth) in all {
            let hwnd = HWND(raw as *mut _);
            let mut buf = [0u16; 64];
            let n = unsafe { GetClassNameW(hwnd, &mut buf) } as usize;
            if n == 0
                || String::from_utf16_lossy(&buf[..n.min(64)])
                    != "Chrome_RenderWidgetHostHWND"
            {
                continue;
            }
            let mut r: RECT = unsafe { std::mem::zeroed() };
            if unsafe { GetWindowRect(hwnd, &mut r) }.is_err() {
                continue;
            }
            let area = (r.right - r.left).max(0) as i64
                * (r.bottom - r.top).max(0) as i64;
            if area > fallback_area {
                fallback_area = area;
                fallback = hwnd;
            }
            if sx >= r.left && sx < r.right && sy >= r.top && sy < r.bottom
                && best.map(|(_, d)| depth > d).unwrap_or(true)
            {
                best = Some((raw, depth));
            }
        }
        best.map(|(raw, _)| HWND(raw as *mut _)).unwrap_or(fallback)
    }

    unsafe extern "system" fn collect_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let out = unsafe { &mut *(lparam.0 as *mut Vec<isize>) };
        out.push(hwnd.0 as isize);
        BOOL(1)
    }

    /// Map a point from one window's client area to another's (via screen).
    pub fn map_point(from: HWND, to: HWND, x: i32, y: i32) -> Option<(i32, i32)> {
        use windows::Win32::Foundation::POINT;
        use windows::Win32::Graphics::Gdi::{ClientToScreen, ScreenToClient};
        unsafe {
            let mut pt = POINT { x, y };
            if !ClientToScreen(from, &mut pt).as_bool() {
                return None;
            }
            if !ScreenToClient(to, &mut pt).as_bool() {
                return None;
            }
            Some((pt.x, pt.y))
        }
    }
}

/// Candidates checked when the frontend sends no (or a bad) exe path.
#[cfg(windows)]
fn candidate_opera_paths() -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        out.push(format!("{local}\\Programs\\Opera\\opera.exe"));
    }
    if let Ok(pf) = std::env::var("ProgramFiles") {
        out.push(format!("{pf}\\Opera\\opera.exe"));
    }
    if let Ok(pfx) = std::env::var("ProgramFiles(x86)") {
        out.push(format!("{pfx}\\Opera\\opera.exe"));
    }
    out
}

/// Resolve which opera.exe to launch. Prefers an explicit existing path,
///
/// then the first existing auto-detected install, then falls back to
/// "opera.exe" (PATH lookup) so the spawn error is still meaningful.
#[cfg(windows)]
fn resolve_opera_exe(requested: Option<String>) -> String {
    if let Some(r) = requested {
        let t = r.trim().to_string();
        if !t.is_empty() {
            if std::path::Path::new(&t).exists() {
                return t;
            }
            // Explicit path doesn't exist — still try auto-detect before failing.
            for c in candidate_opera_paths() {
                if std::path::Path::new(&c).exists() {
                    return c;
                }
            }
            return t; // let spawn report the original bad path
        }
    }
    for c in candidate_opera_paths() {
        if std::path::Path::new(&c).exists() {
            return c;
        }
    }
    "opera.exe".to_string()
}

/// Lets the frontend prefill / display the detected install.
#[tauri::command]
pub fn find_opera_exe() -> Option<String> {
    #[cfg(not(windows))]
    {
        return None;
    }
    #[cfg(windows)]
    {
        for c in candidate_opera_paths() {
            if std::path::Path::new(&c).exists() {
                return Some(c);
            }
        }
        None
    }
}

/// Shared dock worker: find the spawned PID's window, strip its frame,
/// parent it into the Tauri window, glue it to the pending frontend rect,
/// and reap it on exit. Used by both the Opera launcher (Chromium flags)
/// and the generic extra-app launcher (plain spawn).
#[cfg(windows)]
fn track_embedded_window<R: Runtime>(
    app: AppHandle<R>,
    parent_isize: isize,
    mut child: std::process::Child,
) -> Result<u32, String> {
    use tauri::Emitter;
    use win::{find_hwnd_for_pid, place, strip_frame};
    use windows::Win32::UI::WindowsAndMessaging::SetParent;

    let pid = child.id();
    // Publish for section mapping (stream capture uses ClientToScreen).
    if let Ok(mut g) = app.state::<EmbedState>().0.lock() {
        g.parent = Some(parent_isize);
    }
    let app_handle = app.clone();
    std::thread::spawn(move || {
        use windows::Win32::Foundation::HWND;
        let parent = HWND(parent_isize as *mut _);
        // Poll up to ~15s for the window to appear (cold start is slow).
        for _ in 0..150 {
            std::thread::sleep(Duration::from_millis(100));
            if let Some(hwnd) = find_hwnd_for_pid(pid) {
                unsafe {
                    strip_frame(hwnd);
                    let _ = SetParent(hwnd, parent);
                    // Apply the latest frontend rect immediately — early
                    // resize calls are cached in pending so nothing is lost.
                    let (x, y, w, h) = app_handle
                        .state::<EmbedState>()
                        .0
                        .lock()
                        .ok()
                        .and_then(|g| g.pending)
                        .map(|(x, y, w, h, s, _, _)| {
                            (
                                (x as f64 * s) as i32,
                                (y as f64 * s) as i32,
                                (w as f64 * s) as i32,
                                (h as f64 * s) as i32,
                            )
                        })
                        .unwrap_or((0, 200, 800, 500));
                    place(hwnd, x, y, w, h);
                    // Nudge again after the first paint settles.
                    std::thread::sleep(Duration::from_millis(600));
                    place(hwnd, x, y, w, h);
                }
                if let Ok(mut guard) = app_handle.state::<EmbedState>().0.lock() {
                    guard.window = Some((hwnd.0 as isize, pid));
                }
                let _ = app_handle.emit("opera-embedded", pid);
                break;
            }
        }
        // Reap the process handle so we don't leak a zombie.
        let _ = child.wait();
        // Clear state on exit so relaunch works.
        if let Ok(mut guard) = app_handle.state::<EmbedState>().0.lock() {
            if guard.window.map(|(_, p)| p) == Some(pid) {
                guard.window = None;
            }
        }
        let _ = app_handle.emit("opera-closed", pid);
    });
    Ok(pid)
}

#[tauri::command]
pub fn launch_opera_embed<R: Runtime>(
    app: AppHandle<R>,
    _state: State<'_, EmbedState>,
    exe: Option<String>,
) -> Result<String, String> {
    #[cfg(not(windows))]
    {
        let _ = (&app, &_state, &exe);
        return Err("Window embedding is only supported on Windows.".into());
    }

    #[cfg(windows)]
    {
        let main = app
            .get_webview_window("main")
            .ok_or("Main window 'main' not found")?;
        // Tauri's HWND may come from a different `windows` crate version than
        // ours (0.58), so don't pass it directly to SetParent. Round-trip
        // through the raw pointer value instead.
        let parent_tauri = main.hwnd().map_err(|e| format!("Failed to get HWND: {e}"))?;
        // HWND (*mut c_void) is not Send, so move only the raw address
        // into the polling thread and rebuild our HWND there.
        let parent_isize = parent_tauri.0 as isize;

        // Auto-discover opera.exe. "opera.exe" alone only works if it's on
        // PATH (usually it isn't). Typical per-user install lives at
        // %LOCALAPPDATA%\Programs\Opera\opera.exe (confirmed on your PC).
        let exe = resolve_opera_exe(exe);

        // Separate profile dir => forces a NEW Opera process we can track.
        // Without this, Opera re-uses an already-running instance and PID
        // tracking fails. Absolute path: the old relative ".opera-embed"
        // resolved against whatever the CWD happened to be, so dev and
        // installed builds silently split profiles (and could fight over the
        // DevTools port). NOTE: first launch after this change starts a
        // FRESH Opera profile; the old relative dir is left behind.
        let profile_arg = app
            .path()
            .app_data_dir()
            .map(|mut d| {
                d.push(".opera-embed");
                let _ = std::fs::create_dir_all(&d);
                format!("--user-data-dir={}", d.to_string_lossy())
            })
            .unwrap_or_else(|_| "--user-data-dir=.opera-embed".to_string());
        let child = std::process::Command::new(&exe)
            .args([
                &profile_arg,
                "--new-window",
                "--no-first-run",
                "--no-default-browser-check",
                // Fewer reasons for Opera to pop something over the game.
                "--disable-notifications",
                // Keep rendering while occluded/covered so the stream never
                // freezes when another window overlaps the app (same flags as
                // window_streamer documents for Opera/Chrome video capture).
                "--disable-backgrounding-occluded-windows",
                "--disable-renderer-backgrounding",
                "--disable-background-timer-throttling",
                "--disable-features=CalculateNativeWinOcclusion",
                // DevTools endpoint for background scroll injection (cdp.rs):
                // Win32 wheel posts are dropped by Chromium while unfocused,
                // CDP mouseWheel goes straight into Blink. No visible effect.
                &crate::cdp::debugging_arg(),
                "about:blank",
            ])
            .spawn()
            .map_err(|e| {
                format!(
                    "Failed to start '{exe}': {e}. Tried auto-detect; if this persists, paste the full path into the exe box (e.g. %LOCALAPPDATA%\\Programs\\Opera\\opera.exe)."
                )
            })?;
        let pid = track_embedded_window(app, parent_isize, child)?;

        Ok(format!("Launched Opera (pid {pid}). Waiting for window to embed…"))
    }
}

/// Split a user-typed argument string the way Windows does (MSVCRT rules):
/// whitespace separates (outside quotes), double quotes group, and a run of
/// n backslashes followed by a quote collapses to n/2 backslashes plus a
/// literal quote (odd n) or a grouping toggle (even n).
/// Game launch lines like `-opt value --path "C:\a b\c"` survive intact.
#[cfg(windows)]
fn split_launch_args(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut in_arg = false;
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                let mut n = 1usize;
                while chars.peek() == Some(&'\\') {
                    chars.next();
                    n += 1;
                }
                if chars.peek() == Some(&'"') {
                    chars.next();
                    cur.extend(std::iter::repeat('\\').take(n / 2));
                    if n % 2 == 1 {
                        cur.push('"');
                    } else {
                        in_quotes = !in_quotes;
                    }
                    in_arg = true;
                } else {
                    cur.extend(std::iter::repeat('\\').take(n));
                    in_arg = true;
                }
            }
            '"' => {
                in_quotes = !in_quotes;
                in_arg = true;
            }
            c if c.is_whitespace() && !in_quotes => {
                if in_arg {
                    out.push(std::mem::take(&mut cur));
                    in_arg = false;
                }
            }
            c => {
                cur.push(c);
                in_arg = true;
            }
        }
    }
    if in_arg {
        out.push(cur);
    }
    out
}

/// Launch any extra app from the Settings list WITHOUT docking it.
///
/// Plain spawn with NO extra flags: Chromium-only switches
/// (--user-data-dir, --remote-debugging-port, anti-backgrounding) live in
/// `launch_opera_embed` and would break native apps/games. Working dir is
/// set to the exe's folder (games often require it). The window opens as a
/// normal window so launchers/logins/menus can be clicked through, then
/// `embed_launched_app` docks it on demand. Whatever ends up docked feeds
/// the existing stream + input pipeline unchanged — use the normal Close
/// button first if something else is already embedded.
#[tauri::command]
pub fn launch_embedded_app<R: Runtime>(
    app: AppHandle<R>,
    _state: State<'_, EmbedState>,
    exe: String,
    args: Option<String>,
    app_id: Option<String>,
    mcp_enabled: Option<bool>,
    mcp_host: Option<String>,
    mcp_port: Option<u16>,
) -> Result<String, String> {
    #[cfg(not(windows))]
    {
        let _ = (&app, &_state, &exe, &args, &app_id, &mcp_enabled, &mcp_host, &mcp_port);
        return Err("Window embedding is only supported on Windows.".into());
    }

    #[cfg(windows)]
    {
        // No parent HWND needed here: docking happens later on demand.
        let exe = crate::commands::normalize_exe_path(&exe);
        if exe.is_empty() {
            return Err("Pick an exe first.".into());
        }
        if !std::path::Path::new(&exe).exists() {
            return Err(format!("File not found: {exe}"));
        }
        // Persist the MCP forwarding settings the frontend sends with the
        // launch (same shape as mcp_configure). Without this write,
        // inject_input_mcp finds mcp_enabled=false and skips every relayed
        // input — "MCP commands not going through".
        let mcp_host = mcp_host
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("127.0.0.1")
            .to_string();
        let mcp_port = mcp_port.unwrap_or(crate::mcp::DEFAULT_MCP_PORT);
        {
            let state = app.state::<EmbedState>();
            let mut g = state.0.lock().map_err(|e| e.to_string())?;
            g.mcp_enabled = mcp_enabled.unwrap_or(false);
            g.mcp_host = mcp_host.clone();
            g.mcp_port = mcp_port;
            g.embedded_app_id = app_id.clone();
        }
        crate::mcp::mcp_log(&format!(
            "[MCP] launch_embedded_app: app_id={app_id:?} enabled={mcp_enabled:?} host={mcp_host} port={mcp_port}"
        ));
        let name = std::path::Path::new(&exe)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("app")
            .to_string();
        let mut cmd = std::process::Command::new(&exe);
        if let Some(dir) = std::path::Path::new(&exe).parent() {
            if !dir.as_os_str().is_empty() {
                cmd.current_dir(dir);
            }
        }
        // Optional per-app launch arguments from the Settings list,
        // split with real Windows quoting rules (not naive whitespace).
        if let Some(a) = args.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            cmd.args(split_launch_args(a));
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Failed to start '{exe}': {e}"))?;
        let pid = child.id();
        // Remember for manual docking; the window stays a normal window
        // until the user clicks Embed App.
        if let Ok(mut g) = app.state::<EmbedState>().0.lock() {
            g.launched = Some((pid, name.clone()));
        }
        // Reap only — docking happens on demand via embed_launched_app.
        let app_handle = app.clone();
        std::thread::spawn(move || {
            use tauri::Emitter;
            let _ = child.wait();
            if let Ok(mut guard) = app_handle.state::<EmbedState>().0.lock() {
                if guard.window.map(|(_, p)| p) == Some(pid) {
                    guard.window = None;
                }
                if guard.launched.as_ref().map(|(p, _)| *p) == Some(pid) {
                    guard.launched = None;
                }
            }
            let _ = app_handle.emit("opera-closed", pid);
        });

        Ok(format!(
            "Launched {name} (pid {pid}) — click through it, then Embed App."
        ))
    }
}

/// Dock the extra app spawned by `launch_embedded_app`.
///
/// Finds the stored PID's window right now (short poll — it should already
/// exist since the user has been looking at it), strips its frame, parents
/// it into the Tauri window and glues it to the pending frontend rect.
/// Emits the same `opera-embedded` event, so status + docking behave like
/// the Opera path from here on.
#[tauri::command]
pub fn embed_launched_app<R: Runtime>(
    app: AppHandle<R>,
    _state: State<'_, EmbedState>,
) -> Result<String, String> {
    #[cfg(not(windows))]
    {
        let _ = (&app, &_state);
        return Err("Window embedding is only supported on Windows.".into());
    }

    #[cfg(windows)]
    {
        use tauri::Emitter;
        use win::{find_hwnd_for_pid, place, strip_frame};
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::SetParent;

        let main = app
            .get_webview_window("main")
            .ok_or("Main window 'main' not found")?;
        let parent_tauri = main.hwnd().map_err(|e| format!("Failed to get HWND: {e}"))?;
        let parent_isize = parent_tauri.0 as isize;
        if let Ok(mut g) = app.state::<EmbedState>().0.lock() {
            g.parent = Some(parent_isize);
        }

        let (pid, name) = app
            .state::<EmbedState>()
            .0
            .lock()
            .ok()
            .and_then(|g| g.launched.clone())
            .ok_or("Launch an app from the Settings list first.")?;

        // Short poll: the window should already exist (the user was just
        // clicking in it), this only covers slow first paints.
        let mut hwnd_opt = None;
        for _ in 0..50 {
            if let Some(hwnd) = find_hwnd_for_pid(pid) {
                hwnd_opt = Some(hwnd);
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let hwnd = hwnd_opt.ok_or_else(|| {
            format!("No window for {name} (pid {pid}) — is it still open?")
        })?;

        unsafe {
            strip_frame(hwnd);
            let _ = SetParent(hwnd, HWND(parent_isize as *mut _));
            let (x, y, w, h) = app
                .state::<EmbedState>()
                .0
                .lock()
                .ok()
                .and_then(|g| g.pending)
                .map(|(x, y, w, h, s, _, _)| {
                    (
                        (x as f64 * s) as i32,
                        (y as f64 * s) as i32,
                        (w as f64 * s) as i32,
                        (h as f64 * s) as i32,
                    )
                })
                .unwrap_or((0, 200, 800, 500));
            place(hwnd, x, y, w, h);
            std::thread::sleep(Duration::from_millis(600));
            place(hwnd, x, y, w, h);
        }
        if let Ok(mut guard) = app.state::<EmbedState>().0.lock() {
            guard.window = Some((hwnd.0 as isize, pid));
        }
        let _ = app.emit("opera-embedded", pid);
        Ok(format!("{name} docked — use the Stream panel to broadcast it."))
    }
}

#[cfg(all(test, windows))]
mod launch_args_tests {
    use super::split_launch_args;

    #[test]
    fn splits_simply() {
        assert_eq!(split_launch_args("-a b --c"), vec!["-a", "b", "--c"]);
        assert!(split_launch_args("   ").is_empty());
        assert!(split_launch_args("").is_empty());
    }

    #[test]
    fn respects_quotes() {
        assert_eq!(
            split_launch_args("-opt value --path \"C:\\a b\\c\""),
            vec!["-opt", "value", "--path", "C:\\a b\\c"]
        );
        assert_eq!(split_launch_args("\"\""), vec![""]);
    }

    #[test]
    fn handles_backslash_escapes() {
        // 2n backslashes + quote -> n backslashes, quote toggles grouping.
        assert_eq!(split_launch_args("a\\\\\"b c\""), vec!["a\\b c"]);
        // 2n+1 backslashes + quote -> n backslashes + literal quote.
        assert_eq!(split_launch_args("a\\\\\\\"b"), vec!["a\\\"b"]);
    }
}

#[tauri::command]
pub fn resize_embedded_app(
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    scale: Option<f64>,
    vw: Option<f64>,
    vh: Option<f64>,
    state: State<'_, EmbedState>,
) -> Result<(), String> {
    #[cfg(not(windows))]
    {
        let _ = (&x, &y, &w, &h, &scale, &vw, &vh, &state);
        return Err("Window embedding is only supported on Windows.".into());
    }
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::HWND;
        use win::place;

        let scale = scale.unwrap_or(1.0).clamp(1.0, 3.0);
        let vw = vw.unwrap_or(0.0);
        let vh = vh.unwrap_or(0.0);
        let mut guard = state.0.lock().map_err(|e| e.to_string())?;
        // Always cache — launch thread applies this the moment HWND appears,
        // and the stream loop re-reads it every frame while scrolling.
        guard.pending = Some((x, y, w, h, scale, vw, vh));
        if let Some((raw, _)) = guard.window {
            let hwnd = HWND(raw as *mut _);
            place(
                hwnd,
                (x as f64 * scale) as i32,
                (y as f64 * scale) as i32,
                (w as f64 * scale) as i32,
                (h as f64 * scale) as i32,
            );
        }
        Ok(())
    }
}

#[tauri::command]
pub fn close_embedded_app(state: State<'_, EmbedState>) -> Result<(), String> {
    #[cfg(not(windows))]
    {
        let _ = &state;
        return Err("Window embedding is only supported on Windows.".into());
    }
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE};

        let hwnd_raw = state
            .0
            .lock()
            .map_err(|e| e.to_string())?
            .window
            .take()
            .map(|(raw, _)| raw);
        if let Some(raw) = hwnd_raw {
            unsafe {
                let _ = PostMessageW(HWND(raw as *mut _), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
            Ok(())
        } else {
            Err("No embedded window is running.".into())
        }
    }
}

#[tauri::command]
pub fn focus_embedded_app(state: State<'_, EmbedState>) -> Result<(), String> {
    #[cfg(not(windows))]
    {
        let _ = &state;
        return Err("Window embedding is only supported on Windows.".into());
    }
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{HWND_TOP, SetForegroundWindow};

        let guard = state.0.lock().map_err(|e| e.to_string())?;
        if let Some((raw, _)) = guard.window {
            let hwnd = HWND(raw as *mut _);
            unsafe {
                use windows::Win32::UI::WindowsAndMessaging::{SetWindowPos, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW};
                use win::place;
                // Re-assert on top of the WebView + re-apply cached bounds.
                if let Some((x, y, w, h, s, _, _)) = guard.pending {
                    place(
                        hwnd,
                        (x as f64 * s) as i32,
                        (y as f64 * s) as i32,
                        (w as f64 * s) as i32,
                        (h as f64 * s) as i32,
                    );
                } else {
                    let _ = SetWindowPos(hwnd, HWND_TOP, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW);
                }
                let _ = SetForegroundWindow(hwnd);
            }
            Ok(())
        } else {
            Err("No embedded window is running.".into())
        }
    }
}

/// Popup warden + foreground journal + input summary, one cheap thread.
///
/// - Foreground journal (500ms poll): logs every foreground change with
///   pid/class/title. Answers "what opened over the game?" after the fact.
/// - Popup warden (every ~2s): visible top-level windows of the embedded
///   Opera PID other than the docked child (Ctrl+N frames, download windows,
///   permission bubbles...) are minimized — non-destructive, keeps the game
///   view clean — and logged once each.
/// - Input summary (every ~30s): one-line health readout in the log panel.
#[cfg(windows)]
pub fn start_watchdog<R: Runtime>(app: AppHandle<R>) {
    use std::collections::HashSet;
    use std::sync::Once;
    use tauri::Emitter;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        std::thread::spawn(move || {
            use windows::Win32::Foundation::HWND;
            use windows::Win32::UI::WindowsAndMessaging::*;
            let mut last_fg: isize = 0;
            let mut quieted: HashSet<isize> = HashSet::new();
            let mut tick: u64 = 0;
            loop {
                std::thread::sleep(std::time::Duration::from_millis(500));
                tick += 1;
                // --- foreground journal ---
                unsafe {
                    let fg = GetForegroundWindow();
                    let raw = fg.0 as isize;
                    if raw != 0 && raw != last_fg {
                        last_fg = raw;
                        let mut pid = 0u32;
                        GetWindowThreadProcessId(fg, Some(&mut pid));
                        let mut cn = [0u16; 64];
                        let n = GetClassNameW(fg, &mut cn) as usize;
                        let cls = String::from_utf16_lossy(&cn[..n.min(64)]);
                        let mut tn = [0u16; 128];
                        let m = GetWindowTextW(fg, &mut tn) as usize;
                        let title: String =
                            String::from_utf16_lossy(&tn[..m.min(128)])
                                .chars()
                                .take(60)
                                .collect();
                        let line = format!(
                            "[Watchdog] foreground -> pid={pid} class={cls} title={title}"
                        );
                        eprintln!("{line}");
                        let _ = app.emit("rust_log", line);
                    }
                }
                // --- popup warden, ~2s ---
                if tick % 4 == 0 {
                    let embedded = app
                        .state::<EmbedState>()
                        .0
                        .lock()
                        .ok()
                        .and_then(|g| g.window);
                    if let Some((emb_raw, pid)) = embedded {
                        for raw in win::top_windows_for_pid(pid) {
                            if raw == emb_raw {
                                continue;
                            }
                            let hwnd = HWND(raw as *mut _);
                            if !unsafe { IsWindowVisible(hwnd).as_bool() } {
                                continue;
                            }
                            unsafe {
                                let _ = ShowWindow(hwnd, SW_MINIMIZE);
                            }
                            if quieted.insert(raw) {
                                let line = format!(
                                    "[Watchdog] minimized stray Opera window 0x{raw:X}"
                                );
                                eprintln!("{line}");
                                let _ = app.emit("rust_log", &line);
                            }
                        }
                        // Prune dead handles so a recycled HWND value gets
                        // logged again instead of staying quiet forever.
                        quieted.retain(|&r| unsafe {
                            IsWindow(HWND(r as *mut _)).as_bool()
                        });
                    }
                }
                // --- input summary, ~30s ---
                if tick % 60 == 0 {
                    use std::sync::atomic::Ordering::Relaxed;
                    let line = format!(
                        "[Watchdog] input packets={} unknown={} cdp_ok={} cdp_err={} fallback={} dropped={} last_err={}",
                        crate::beamng_udp::INPUT_PACKETS.load(Relaxed),
                        crate::beamng_udp::INPUT_UNKNOWN.load(Relaxed),
                        crate::cdp::CDP_OK.load(Relaxed),
                        crate::cdp::CDP_ERR.load(Relaxed),
                        crate::cdp::CDP_FALLBACK.load(Relaxed),
                        crate::cdp::QUEUE_DROPPED.load(Relaxed),
                        crate::cdp::last_err(),
                    );
                    eprintln!("{line}");
                    let _ = app.emit("rust_log", line);
                }
            }
        });
    });
}

#[cfg(not(windows))]
pub fn start_watchdog<R: Runtime>(app: AppHandle<R>) {
    let _ = app;
}

/// Synthesize a left click in the embedded Opera child at normalized 0..1
/// coords (as relayed from the in-game stream view).
///
/// `method = "post"` (default): background Win32 messages — the real cursor
/// never moves and Opera keeps running headless-friendly.
/// `method = "sendinput"`: real input at the point (cursor saved, moved,
/// clicked, restored). Use if Chromium ignores the background messages.
#[cfg(windows)]
pub(crate) fn opera_click(
    child_raw: isize,
    nx: f64,
    ny: f64,
    method: &str,
) -> Result<String, String> {
    use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::*;

    let hwnd = HWND(child_raw as *mut _);
    let (cw, ch) = unsafe {
        let mut r: RECT = std::mem::zeroed();
        if GetClientRect(hwnd, &mut r).is_err() {
            return Err("embedded Opera window is gone".into());
        }
        (r.right - r.left, r.bottom - r.top)
    };
    if cw <= 0 || ch <= 0 {
        return Err("embedded Opera window has no area".into());
    }
    let px = (nx.clamp(0.0, 1.0) * cw as f64).round() as i32;
    let py = (ny.clamp(0.0, 1.0) * ch as f64).round() as i32;
    // MAKELPARAM(x, y)
    let lp = LPARAM(((py << 16) | (px & 0xFFFF)) as isize);

    if method == "sendinput" {
        use windows::Win32::Graphics::Gdi::ClientToScreen;
        use windows::Win32::UI::Input::KeyboardAndMouse::*;
        unsafe {
            let mut pt = POINT { x: px, y: py };
            if !ClientToScreen(hwnd, &mut pt).as_bool() {
                return Err("ClientToScreen failed".into());
            }
            let mut saved = POINT { x: 0, y: 0 };
            let have_saved = GetCursorPos(&mut saved).is_ok();
            let _ = SetForegroundWindow(hwnd);
            if SetCursorPos(pt.x, pt.y).is_err() {
                return Err("SetCursorPos failed".into());
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
            let down = INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT {
                        dx: 0,
                        dy: 0,
                        mouseData: 0,
                        dwFlags: MOUSEEVENTF_LEFTDOWN,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            };
            SendInput(&[down], std::mem::size_of::<INPUT>() as i32);
            std::thread::sleep(std::time::Duration::from_millis(30));
            let up = INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT {
                        dx: 0,
                        dy: 0,
                        mouseData: 0,
                        dwFlags: MOUSEEVENTF_LEFTUP,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            };
            SendInput(&[up], std::mem::size_of::<INPUT>() as i32);
            if have_saved {
                let _ = SetCursorPos(saved.x, saved.y);
            }
        }
        return Ok(format!("sendinput click at {px},{py} in {cw}x{ch}"));
    }

    // Default: background messages — cursor never moves.
    // NOTE: no SetForegroundWindow here on purpose. The Opera child is
    // SetParent'ed into the Tauri window, so foregrounding it activates the
    // whole app and pops it over the game on every in-game click.
    // See FOCUS_STEAL_FIX_REVERT.md to restore the old behavior.
    unsafe {
        let _ = PostMessageW(hwnd, WM_MOUSEMOVE, WPARAM(0), lp);
        // MK_LBUTTON = 0x0001
        let _ = PostMessageW(hwnd, WM_LBUTTONDOWN, WPARAM(1), lp);
        std::thread::sleep(std::time::Duration::from_millis(30));
        let _ = PostMessageW(hwnd, WM_LBUTTONUP, WPARAM(0), lp);
    }
    Ok(format!("posted click at {px},{py} in {cw}x{ch}"))
}

#[cfg(not(windows))]
pub(crate) fn opera_click(
    _child_raw: isize,
    _nx: f64,
    _ny: f64,
    _method: &str,
) -> Result<String, String> {
    Err("Opera click injection is only supported on Windows.".into())
}

/// Full pointer-input relay for the stream view: move / down / up / wheel.
///
/// Coordinates are normalized 0..1 of the streamed Opera frame (which matches
/// the child content); `button` is left|right|middle; `delta` is wheel units
/// (Windows convention: negative = scroll down); `held` is the JS e.buttons
/// bitmask (1/2/4) mapped to MK_* for wParam so drags track correctly.
/// Same `method` choice as [`opera_click`]. Taps arrive as down+up pairs and
/// double-taps form double-clicks from timing — no separate dblclick message.
#[cfg(windows)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn opera_input(
    child_raw: isize,
    action: &str,
    button: &str,
    nx: f64,
    ny: f64,
    delta: i32,
    held: i32,
    method: &str,
) -> Result<String, String> {
    use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::*;

    fn mk_mask(js_buttons: i32) -> usize {
        let mut m = 0usize;
        if js_buttons & 1 != 0 {
            m |= 0x0001;
        } // left
        if js_buttons & 2 != 0 {
            m |= 0x0002;
        } // right
        if js_buttons & 4 != 0 {
            m |= 0x0010;
        } // middle
        m
    }

    let hwnd = HWND(child_raw as *mut _);
    let (cw, ch) = unsafe {
        let mut r: RECT = std::mem::zeroed();
        if GetClientRect(hwnd, &mut r).is_err() {
            return Err("embedded Opera window is gone".into());
        }
        (r.right - r.left, r.bottom - r.top)
    };
    if cw <= 0 || ch <= 0 {
        return Err("embedded Opera window has no area".into());
    }
    let px = (nx.clamp(0.0, 1.0) * cw as f64).round() as i32;
    let py = (ny.clamp(0.0, 1.0) * ch as f64).round() as i32;
    // MAKELPARAM(x, y)
    let lp = LPARAM(((py << 16) | (px & 0xFFFF)) as isize);
    let mask = mk_mask(held);

    // (post down msg, post up msg, wparam bit, label)
    let btn: (u32, u32, usize, &str) = match button {
        "right" => (WM_RBUTTONDOWN, WM_RBUTTONUP, 0x0002, "right"),
        "middle" => (WM_MBUTTONDOWN, WM_MBUTTONUP, 0x0010, "middle"),
        _ => (WM_LBUTTONDOWN, WM_LBUTTONUP, 0x0001, "left"),
    };

    if method == "sendinput" {
        use windows::Win32::UI::Input::KeyboardAndMouse::*;
        unsafe {
            let mut pt = POINT { x: px, y: py };
            if !windows::Win32::Graphics::Gdi::ClientToScreen(hwnd, &mut pt).as_bool() {
                return Err("ClientToScreen failed".into());
            }
            // ABSOLUTE mouse coords span the virtual screen (all monitors).
            let vx = GetSystemMetrics(SM_XVIRTUALSCREEN);
            let vy = GetSystemMetrics(SM_YVIRTUALSCREEN);
            let vw = GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1);
            let vh = GetSystemMetrics(SM_CYVIRTUALSCREEN).max(1);
            let ax = (((pt.x - vx) as i64 * 65535) / vw as i64).clamp(0, 65535) as i32;
            let ay = (((pt.y - vy) as i64 * 65535) / vh as i64).clamp(0, 65535) as i32;
            let _ = SetForegroundWindow(hwnd);
            let abs_move = INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT {
                        dx: ax,
                        dy: ay,
                        mouseData: 0,
                        dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            };
            let sz = std::mem::size_of::<INPUT>() as i32;
            let btn_flag = |down: bool| match btn.3 {
                "right" => {
                    if down {
                        MOUSEEVENTF_RIGHTDOWN
                    } else {
                        MOUSEEVENTF_RIGHTUP
                    }
                }
                "middle" => {
                    if down {
                        MOUSEEVENTF_MIDDLEDOWN
                    } else {
                        MOUSEEVENTF_MIDDLEUP
                    }
                }
                _ => {
                    if down {
                        MOUSEEVENTF_LEFTDOWN
                    } else {
                        MOUSEEVENTF_LEFTUP
                    }
                }
            };
            let mouse = |flags: MOUSE_EVENT_FLAGS, data: u32| INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT {
                        dx: ax,
                        dy: ay,
                        mouseData: data,
                        dwFlags: flags,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            };
            match action {
                "move" => {
                    SendInput(&[abs_move], sz);
                }
                "down" => {
                    SendInput(&[abs_move, mouse(btn_flag(true), 0)], sz);
                }
                "up" => {
                    SendInput(&[mouse(btn_flag(false), 0)], sz);
                }
                "wheel" => {
                    SendInput(&[abs_move, mouse(MOUSEEVENTF_WHEEL, delta as u32)], sz);
                }
                _ => return Err(format!("unknown input action '{action}'")),
            }
        }
        return Ok(format!("sendinput {action} {} at {px},{py}", btn.3));
    }

    // Default: background messages — cursor never moves.
    // NOTE: no SetForegroundWindow here on purpose (focus-steal fix).
    // Foregrounding the reparented child brings the whole Tauri app to the
    // front over BeamNG. Only the sendinput path above may foreground.
    // See FOCUS_STEAL_FIX_REVERT.md to restore the old behavior.
    unsafe {
        match action {
            "move" => {
                let _ = PostMessageW(hwnd, WM_MOUSEMOVE, WPARAM(mask), lp);
            }
            "down" => {
                let _ = PostMessageW(hwnd, btn.0, WPARAM(mask | btn.2), lp);
            }
            "up" => {
                let _ = PostMessageW(hwnd, btn.1, WPARAM(mask), lp);
            }
            "wheel" => {
                use windows::Win32::Graphics::Gdi::ClientToScreen;
                // Wheel must enter through the TOP frame, exactly like the
                // clicks/keys that provably work: Chromium's frame forwards
                // those through its input router (which also solves iframe
                // hosts), while wheel posts sent straight to a render host
                // arrive with no routing context and are dropped. Two
                // wheel-specific needs on top of the click path: (1) a hover
                // MOVE first so the router knows where to scroll, (2) full
                // WHEEL_DELTA (120) notches — the mod sends ~100/notch, so
                // bank partials across events. Still no SetForegroundWindow.
                static WHEEL_REM: std::sync::atomic::AtomicI32 =
                    std::sync::atomic::AtomicI32::new(0);
                let acc = WHEEL_REM
                    .fetch_add(delta, std::sync::atomic::Ordering::Relaxed)
                    .wrapping_add(delta);
                // Truncates toward zero, so mixed directions cancel correctly.
                let notches = acc / 120;
                if notches != 0 {
                    WHEEL_REM.fetch_sub(
                        notches * 120,
                        std::sync::atomic::Ordering::Relaxed,
                    );
                }
                if notches == 0 {
                    return Ok(format!("banked wheel {delta} (remainder {acc})"));
                }
                let stepped = notches * 120;
                // Hand off to the CDP delivery worker (see cdp.rs): the UDP
                // thread must never block on network. NOTE: unlike the other
                // mouse messages, WM_MOUSEWHEEL takes SCREEN coords.
                let mut pt = POINT { x: px, y: py };
                if !ClientToScreen(hwnd, &mut pt).as_bool() {
                    return Err("ClientToScreen failed".into());
                }
                let job = crate::cdp::WheelJob {
                    nx,
                    ny,
                    cw,
                    ch,
                    stepped,
                    child_raw,
                    mask,
                    px,
                    py,
                    sx: pt.x,
                    sy: pt.y,
                };
                return match crate::cdp::queue_wheel(job) {
                    Ok(()) => Ok(format!("queued wheel {stepped} at {px},{py}")),
                    Err(()) => Ok(format!(
                        "dropped wheel {stepped} (delivery overloaded)"
                    )),
                };
            }
            _ => return Err(format!("unknown input action '{action}'")),
        }
    }
    Ok(format!("posted {action} {} at {px},{py} in {cw}x{ch}", btn.3))
}

/// Win32 fallback for wheel delivery, run on the CDP worker thread — never on
/// the UDP fast path. Hover MOVE (client coords) + `WM_MOUSEWHEEL` (screen
/// coords) under focus-without-foreground. Chromium usually drops these while
/// unfocused, which is why CDP is primary; this path exists for embedded
/// instances that predate the DevTools flag.
#[cfg(windows)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn post_wheel_fallback(
    child_raw: isize,
    mask: usize,
    px: i32,
    py: i32,
    sx: i32,
    sy: i32,
    stepped: i32,
) -> Result<String, String> {
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::*;
    let hwnd = HWND(child_raw as *mut _);
    let lp = LPARAM(((py << 16) | (px & 0xFFFF)) as isize);
    let wlp = LPARAM(((sy << 16) | (sx & 0xFFFF)) as isize);
    let wp = WPARAM(((stepped as u32) << 16) as usize);
    unsafe {
        let _focus = win::FocusGuard::focus(hwnd);
        let _ = PostMessageW(hwnd, WM_MOUSEMOVE, WPARAM(mask), lp);
        let _ = PostMessageW(hwnd, WM_MOUSEWHEEL, wp, wlp);
    }
    Ok(format!("posted wheel {stepped} at {px},{py} focused"))
}

#[cfg(not(windows))]
#[allow(clippy::too_many_arguments)]
pub(crate) fn opera_input(
    _child_raw: isize,
    _action: &str,
    _button: &str,
    _nx: f64,
    _ny: f64,
    _delta: i32,
    _held: i32,
    _method: &str,
) -> Result<String, String> {
    Err("Opera input injection is only supported on Windows.".into())
}

/// Physical-key relay for screen typing: `code` is the layout-independent
/// physical code (KeyW, Digit1, Space, Enter, ...), `key` the resolved
/// printable char if any (already shift-aware from the sender).
///
/// `down` true = press, false = release. Hardware-faithful: KEYDOWN/KEYUP
/// with a real scan code + extended flag, and NO explicit WM_CHAR — the
/// target thread's TranslateMessage produces exactly one WM_CHAR from the
/// KEYDOWN (posting our own WM_CHAR as well doubled every printable).
/// Same `method` choice as pointer input; sendinput uses real KEYBDINPUT.
#[cfg(windows)]
pub(crate) fn opera_key(
    child_raw: isize,
    code: &str,
    key: &str,
    down: bool,
    method: &str,
) -> Result<String, String> {
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
    use windows::Win32::UI::WindowsAndMessaging::*;

    fn vk_from_code(code: &str) -> Option<VIRTUAL_KEY> {
        // Physical codes: KeyA..KeyZ == VK 0x41..0x5A, Digit0..9 == 0x30..0x39.
        let b = code.as_bytes();
        if b.len() == 4 && b[0] == b'K' && b[1] == b'e' && b[2] == b'y' && b[3].is_ascii_uppercase()
        {
            return Some(VIRTUAL_KEY(b[3] as u16));
        }
        if b.len() == 6 && &b[0..5] == b"Digit" && b[5].is_ascii_digit() {
            return Some(VIRTUAL_KEY(b[5] as u16));
        }
        Some(match code {
            "Enter" | "NumpadEnter" => VK_RETURN,
            "Backspace" => VK_BACK,
            "Tab" => VK_TAB,
            "Space" => VK_SPACE,
            "Escape" => VK_ESCAPE,
            "CapsLock" => VK_CAPITAL,
            "Delete" => VK_DELETE,
            "Insert" => VK_INSERT,
            "Home" => VK_HOME,
            "End" => VK_END,
            "PageUp" => VK_PRIOR,
            "PageDown" => VK_NEXT,
            "ArrowLeft" => VK_LEFT,
            "ArrowUp" => VK_UP,
            "ArrowRight" => VK_RIGHT,
            "ArrowDown" => VK_DOWN,
            "ShiftLeft" => VK_LSHIFT,
            "ShiftRight" => VK_RSHIFT,
            "ControlLeft" => VK_LCONTROL,
            "ControlRight" => VK_RCONTROL,
            "AltLeft" => VK_LMENU,
            "AltRight" => VK_RMENU,
            "MetaLeft" => VK_LWIN,
            "MetaRight" => VK_RWIN,
            "ContextMenu" => VK_APPS,
            "PrintScreen" => VK_SNAPSHOT,
            "ScrollLock" => VK_SCROLL,
            "Pause" => VK_PAUSE,
            "NumLock" => VK_NUMLOCK,
            "F1" => VK_F1,
            "F2" => VK_F2,
            "F3" => VK_F3,
            "F4" => VK_F4,
            "F5" => VK_F5,
            "F6" => VK_F6,
            "F7" => VK_F7,
            "F8" => VK_F8,
            "F9" => VK_F9,
            "F10" => VK_F10,
            "F11" => VK_F11,
            "F12" => VK_F12,
            "Numpad0" => VK_NUMPAD0,
            "Numpad1" => VK_NUMPAD1,
            "Numpad2" => VK_NUMPAD2,
            "Numpad3" => VK_NUMPAD3,
            "Numpad4" => VK_NUMPAD4,
            "Numpad5" => VK_NUMPAD5,
            "Numpad6" => VK_NUMPAD6,
            "Numpad7" => VK_NUMPAD7,
            "Numpad8" => VK_NUMPAD8,
            "Numpad9" => VK_NUMPAD9,
            "NumpadAdd" => VK_ADD,
            "NumpadSubtract" => VK_SUBTRACT,
            "NumpadMultiply" => VK_MULTIPLY,
            "NumpadDivide" => VK_DIVIDE,
            "NumpadDecimal" => VK_DECIMAL,
            "Semicolon" => VK_OEM_1,
            "Equal" => VK_OEM_PLUS,
            "Comma" => VK_OEM_COMMA,
            "Minus" => VK_OEM_MINUS,
            "Period" => VK_OEM_PERIOD,
            "Slash" => VK_OEM_2,
            "Backquote" => VK_OEM_3,
            "BracketLeft" => VK_OEM_4,
            "Backslash" => VK_OEM_5,
            "BracketRight" => VK_OEM_6,
            "Quote" => VK_OEM_7,
            _ => return None,
        })
    }

    // Fallback for bare characters when no code arrived.
    fn vk_from_key(key: &str) -> Option<VIRTUAL_KEY> {
        let mut it = key.chars();
        let c = it.next()?;
        if it.next().is_some() {
            return None;
        }
        if c.is_ascii_alphabetic() {
            return Some(VIRTUAL_KEY(c.to_ascii_uppercase() as u16));
        }
        if c.is_ascii_digit() {
            return Some(VIRTUAL_KEY(c as u16));
        }
        if c == ' ' {
            return Some(VK_SPACE);
        }
        None
    }

    let vk = vk_from_code(code)
        .or_else(|| {
            if key.is_empty() {
                None
            } else {
                vk_from_key(key)
            }
        })
        .ok_or_else(|| format!("unknown key '{code}'"))?;
    let hwnd = HWND(child_raw as *mut _);

    if method == "sendinput" {
        unsafe {
            let _ = SetForegroundWindow(hwnd);
            let v = vk.0;
            // Extended-key flag for navigation/editing keys (numeric VK ranges).
            let extended = matches!(
                v,
                0x25..=0x28 | 0x24 | 0x23 | 0x21 | 0x22 | 0x2D | 0x2E | 0xA3 | 0xA5 | 0x2C | 0x6F | 0x90
            );
            let mut flags = if down {
                KEYBD_EVENT_FLAGS(0)
            } else {
                KEYEVENTF_KEYUP
            };
            if extended {
                flags = flags | KEYEVENTF_EXTENDEDKEY;
            }
            let ki = KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            };
            let inp = INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 { ki },
            };
            SendInput(&[inp], std::mem::size_of::<INPUT>() as i32);
        }
        return Ok(format!(
            "sendinput key {code} {}",
            if down { "down" } else { "up" }
        ));
    }

    // Default: background messages that mimic real hardware as closely as
    // possible — KEYDOWN/KEYUP with a real scan code + extended flag, and no
    // explicit WM_CHAR. The target thread's TranslateMessage turns the
    // KEYDOWN into exactly one WM_CHAR (using the user's real shift/layout
    // state). Posting WM_CHAR ourselves as well doubled every printable:
    // the KEYDOWN got translated into a second CHAR on top of ours.
    // (Backspace/controls were unaffected because a scan-less KEYDOWN does
    // not translate into a CHAR for them — which is also why only letters
    // doubled.)
    // NOTE: no SetForegroundWindow here on purpose (focus-steal fix).
    // Every in-game keystroke used to foreground the Opera child, which
    // activates the parent Tauri window and pops the app over BeamNG.
    // Only the sendinput path above still foregrounds (it needs it).
    // See FOCUS_STEAL_FIX_REVERT.md to restore the old behavior.
    unsafe {
        let scan = MapVirtualKeyW(vk.0 as u32, MAPVK_VK_TO_VSC);
        let v = vk.0;
        let extended = matches!(
            v,
            0x25..=0x28 | 0x24 | 0x23 | 0x21 | 0x22 | 0x2D | 0x2E | 0xA3 | 0xA5 | 0x2C | 0x6F | 0x90
        );
        // lParam: repeat count (bit 0) | scan code (bits 16-23) | extended (24).
        let base: isize = 0x00000001 | ((scan as isize) << 16) | (if extended { 1 << 24 } else { 0 });
        if down {
            let _ = PostMessageW(hwnd, WM_KEYDOWN, WPARAM(vk.0 as usize), LPARAM(base));
        } else {
            // KEYUP sets previous-state (30) + transition (31) bits.
            let _ = PostMessageW(
                hwnd,
                WM_KEYUP,
                WPARAM(vk.0 as usize),
                LPARAM(base | (1 << 30) | (1 << 31)),
            );
        }
    }
    Ok(format!(
        "posted key {code} {}",
        if down { "down" } else { "up" }
    ))
}

#[cfg(not(windows))]
pub(crate) fn opera_key(
    _child_raw: isize,
    _code: &str,
    _key: &str,
    _down: bool,
    _method: &str,
) -> Result<String, String> {
    Err("Opera key injection is only supported on Windows.".into())
}
