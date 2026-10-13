use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(target_os = "windows")]
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession,
    GlobalSystemMediaTransportControlsSessionManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus,
};

// Fix #9: Added Clone derive for Emitter::emit() compatibility (Tauri API requires Serialize + Clone)
#[derive(Debug, Clone, Serialize)]
pub(crate) struct MediaInfo {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub is_playing: bool,
    pub current_time: u64,
    pub total_time: u64,
    pub thumbnail: Option<String>,
}

impl Default for MediaInfo {
    fn default() -> Self {
        Self {
            title: "No Media".to_string(),
            artist: "".to_string(),
            album: "".to_string(),
            is_playing: false,
            current_time: 0,
            total_time: 0,
            thumbnail: None,
        }
    }
}

// Fix #9: Cached media info to reduce SMTC calls
// Using RwLock for better concurrent read performance
struct CachedMediaInfo {
    info: RwLock<MediaInfo>,
    last_fetch: AtomicU64,  // Store as milliseconds since epoch
    cache_duration_ms: u64,
}

impl CachedMediaInfo {
    fn new(cache_duration_ms: u64) -> Self {
        Self {
            info: RwLock::new(MediaInfo::default()),
            last_fetch: AtomicU64::new(0),
            cache_duration_ms,
        }
    }
    
    fn is_stale(&self) -> bool {
        let last = self.last_fetch.load(Ordering::Relaxed);
        if last == 0 { return true; }
        
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        
        now - last > self.cache_duration_ms
    }
    
    fn update(&self, info: MediaInfo) {
        *self.info.write().unwrap() = info;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        self.last_fetch.store(now, Ordering::Relaxed);
    }
    
    fn get(&self) -> MediaInfo {
        self.info.read().unwrap().clone()
    }

    fn invalidate(&self) {
        self.last_fetch.store(0, Ordering::Relaxed);
    }

    // Force fresh fetch bypassing cache - for polling thread
    fn force_fetch() -> MediaInfo {
        #[cfg(target_os = "windows")]
        {
            std::thread::spawn(|| get_media_info_from_smtc_blocking())
                .join()
                .unwrap_or_default()
        }
        #[cfg(not(target_os = "windows"))]
        {
            MediaInfo::default()
        }
    }
}

// Global cached media info (2000ms cache for faster updates)
static CACHED_MEDIA_INFO: OnceLock<Arc<CachedMediaInfo>> = OnceLock::new();

fn get_cached_media_info() -> &'static Arc<CachedMediaInfo> {
    CACHED_MEDIA_INFO.get_or_init(|| Arc::new(CachedMediaInfo::new(2000)))
}

#[cfg(target_os = "windows")]
fn read_thumbnail_to_base64(
    thumb_ref: &windows::Storage::Streams::IRandomAccessStreamReference,
) -> Option<String> {
    let stream = thumb_ref.OpenReadAsync().ok()?.get().ok()?;
    let size = stream.Size().ok()? as u32;
    let reader = windows::Storage::Streams::DataReader::CreateDataReader(&stream).ok()?;
    reader.LoadAsync(size).ok()?.get().ok()?;
    let mut bytes = vec![0u8; size as usize];
    reader.ReadBytes(&mut bytes).ok()?;
    let base64_data =
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &bytes);
    let content_type = if bytes.len() > 1 {
        if bytes[0] == 0xFF && bytes[1] == 0xD8 {
            "image/jpeg"
        } else if bytes[0] == 0x89 && bytes[1] == 0x50 {
            "image/png"
        } else {
            "image/jpeg"
        }
    } else {
        "image/jpeg"
    };
    Some(format!("data:{};base64,{}", content_type, base64_data))
}

#[cfg(not(target_os = "windows"))]
fn read_thumbnail_to_base64<T>(_thumb_ref: &T) -> Option<String> {
    None
}

#[cfg(target_os = "windows")]
fn find_best_media_session(
    manager: &GlobalSystemMediaTransportControlsSessionManager,
) -> Option<GlobalSystemMediaTransportControlsSession> {
    let sessions = manager.GetSessions().ok()?;
    let mut best_session: Option<GlobalSystemMediaTransportControlsSession> = None;
    let session_count = sessions.Size().unwrap_or(0);

    for i in 0..session_count {
        if let Ok(session) = sessions.GetAt(i) {
            let has_media = session
                .TryGetMediaPropertiesAsync()
                .ok()
                .and_then(|op| op.get().ok())
                .and_then(|props| props.Title().ok())
                .map(|h| !h.is_empty())
                .unwrap_or(false);

            let is_playing = session
                .GetPlaybackInfo()
                .ok()
                .and_then(|info| info.PlaybackStatus().ok())
                .map(|s| {
                    s == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing
                })
                .unwrap_or(false);

            if is_playing {
                return Some(session);
            } else if has_media && best_session.is_none() {
                best_session = Some(session);
            }
        }
    }

    best_session
}

// Remembers which session (by SourceAppUserModelId) was last displayed/controlled
// so that transport commands target the same app even when it is paused and
// another stale session would otherwise be selected.
#[cfg(target_os = "windows")]
static LAST_SESSION_ID: OnceLock<RwLock<Option<String>>> = OnceLock::new();

#[cfg(target_os = "windows")]
fn last_session_id() -> &'static RwLock<Option<String>> {
    LAST_SESSION_ID.get_or_init(|| RwLock::new(None))
}

#[cfg(target_os = "windows")]
fn remember_session(session: &GlobalSystemMediaTransportControlsSession) {
    let id = session
        .SourceAppUserModelId()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_default();
    if !id.is_empty() {
        *last_session_id().write().unwrap() = Some(id);
    }
}

#[cfg(target_os = "windows")]
fn find_target_session(
    manager: &GlobalSystemMediaTransportControlsSessionManager,
) -> Option<GlobalSystemMediaTransportControlsSession> {
    let remembered = last_session_id().read().unwrap().clone();
    if let Some(id) = remembered {
        if let Ok(sessions) = manager.GetSessions() {
            let session_count = sessions.Size().unwrap_or(0);
            for i in 0..session_count {
                if let Ok(session) = sessions.GetAt(i) {
                    let matches = session
                        .SourceAppUserModelId()
                        .map(|h| h.to_string_lossy().to_string() == id)
                        .unwrap_or(false);
                    if matches {
                        return Some(session);
                    }
                }
            }
        }
    }
    find_best_media_session(manager)
}

#[cfg(target_os = "windows")]
fn get_media_info_from_smtc_blocking() -> MediaInfo {
    let manager = match GlobalSystemMediaTransportControlsSessionManager::RequestAsync()
        .ok()
        .and_then(|op| op.get().ok())
    {
        Some(m) => m,
        None => return MediaInfo::default(),
    };

    let session = match find_best_media_session(&manager) {
        Some(s) => s,
        None => {
            eprintln!("[MediaInfo] No media session found");
            return MediaInfo::default();
        }
    };
    
    // Debug: log session info
    if let Ok(id) = session.SourceAppUserModelId() {
        eprintln!("[MediaInfo] Selected session: {}", id.to_string_lossy());
    }
    remember_session(&session);

    let props = match session
        .TryGetMediaPropertiesAsync()
        .ok()
        .and_then(|op| op.get().ok())
    {
        Some(p) => p,
        None => return MediaInfo::default(),
    };

    let title = props
        .Title()
        .ok()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_default();
    let artist = props
        .Artist()
        .ok()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_default();
    let album = props
        .AlbumTitle()
        .ok()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_default();
    let thumbnail = props.Thumbnail().ok().and_then(|t| read_thumbnail_to_base64(&t));

    let is_playing = session
        .GetPlaybackInfo()
        .ok()
        .and_then(|info| info.PlaybackStatus().ok())
        .map(|s| s == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing)
        .unwrap_or(false);

    // Try to get timeline properties for position/duration
    let (mut current_time, mut total_time) = (0u64, 0u64);
    if let Ok(timeline) = session.GetTimelineProperties() {
        let start_ticks = timeline.StartTime().map(|ts| ts.Duration).unwrap_or(0);
        let end_ticks = timeline.EndTime().map(|ts| ts.Duration).unwrap_or(0);
        let position_ticks = timeline.Position().map(|ts| ts.Duration).unwrap_or(0);
        total_time = ((end_ticks - start_ticks) / 10_000_000) as u64;
        current_time = ((position_ticks - start_ticks) / 10_000_000) as u64;
        
        eprintln!("[MediaInfo] Timeline OK: pos={} total={}", current_time, total_time);
    } else {
        eprintln!("[MediaInfo] Timeline NOT available for session");
    }

    MediaInfo {
        title: if title.is_empty() { "No Media".to_string() } else { title },
        artist: if artist.is_empty() { "Unknown Artist".to_string() } else { artist },
        album,
        is_playing,
        current_time,
        total_time,
        thumbnail,
    }
}

#[cfg(not(target_os = "windows"))]
fn get_media_info_from_smtc_blocking() -> MediaInfo {
    MediaInfo::default()
}

#[tauri::command]
pub async fn get_media_info() -> Result<MediaInfo, String> {
    let cached = get_cached_media_info();

    // Fix #9: Check cache first (read lock is fast and non-blocking for reads)
    if !cached.is_stale() {
        return Ok(cached.get());
    }

    // Clone Arc before moving into spawn_blocking to avoid borrow-after-move
    let cached_clone = Arc::clone(&cached);

    // Fix #9: Run blocking SMTC call on background thread to avoid blocking command queue
    let result = tokio::task::spawn_blocking(move || {
        #[cfg(target_os = "windows")]
        {
            let new_info = std::thread::spawn(|| get_media_info_from_smtc_blocking())
                .join()
                .unwrap_or_default();

            // Update cache
            cached_clone.update(new_info.clone());
            return new_info;
        }

        #[cfg(not(target_os = "windows"))]
        {
            let info = MediaInfo::default();
            cached_clone.update(info.clone());
            return info;
        }
    })
    .await
    .map_err(|e| format!("Task failed: {}", e))
    .unwrap_or_default();

    Ok(result)
}

// Force fresh media info fetch (bypasses cache) - for polling thread
#[tauri::command]
pub async fn get_media_info_fresh() -> Result<MediaInfo, String> {
    Ok(tokio::task::spawn_blocking(move || {
        CachedMediaInfo::force_fetch()
    })
    .await
    .map_err(|e| format!("Task failed: {}", e))
    .unwrap_or_default())
}

#[cfg(target_os = "windows")]
fn send_smtc_command_blocking(command: MediaCommand) -> Result<(), String> {
    use windows::Media::Control::GlobalSystemMediaTransportControlsSessionManager;

    let manager = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()
        .map_err(|e| format!("Failed to request SMTC manager: {}", e))?
        .get()
        .map_err(|e| format!("Failed to get SMTC manager: {}", e))?;

    let session = find_target_session(&manager)
        .ok_or_else(|| "No active media session found".to_string())?;

    // Verify the player actually supports the requested command
    let controls = session
        .GetPlaybackInfo()
        .ok()
        .and_then(|info| info.Controls().ok());
    if let Some(c) = &controls {
        let supported = match command {
            MediaCommand::PlayPause => {
                c.IsPlayEnabled().unwrap_or(false) || c.IsPauseEnabled().unwrap_or(false)
            }
            MediaCommand::Next => c.IsNextEnabled().unwrap_or(false),
            MediaCommand::Previous => c.IsPreviousEnabled().unwrap_or(false),
        };
        if !supported {
            return match command {
                MediaCommand::Next => send_browser_skip_keys(true),
                MediaCommand::Previous => send_browser_skip_keys(false),
                MediaCommand::PlayPause => Err(
                    "This command is not supported by the active media app".to_string(),
                ),
            };
        }
    }

    let result = match command {
        MediaCommand::PlayPause => session.TryTogglePlayPauseAsync(),
        MediaCommand::Next => session.TrySkipNextAsync(),
        MediaCommand::Previous => session.TrySkipPreviousAsync(),
    }
    .map_err(|e| format!("Failed to send command: {}", e))?
    .get()
    .map_err(|e| format!("Command failed: {}", e))?;

    // Invalidate cache so the next get_media_info call fetches fresh state
    get_cached_media_info().invalidate();

    let _ = result;
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn send_smtc_command_blocking(_command: MediaCommand) -> Result<(), String> {
    Err("Media control is only supported on Windows".to_string())
}

#[derive(Debug, Clone, Copy)]
enum MediaCommand {
    PlayPause,
    Next,
    Previous,
}

// Fallback for browsers that don't expose next/previous via SMTC (e.g. YouTube
// videos in Opera/Chrome): focus the browser window and send the YouTube
// shortcuts Shift+N (next) / Shift+P (previous), then restore focus.
#[cfg(target_os = "windows")]
fn find_browser_window() -> Option<windows::Win32::Foundation::HWND> {
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetClassNameW, GetWindowTextLengthW, GetWindowTextW, IsWindowVisible,
    };

    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let sink = lparam.0 as *mut Vec<isize>;
        (*sink).push(hwnd.0 as isize);
        true.into()
    }

    let mut windows_found: Vec<isize> = Vec::new();
    unsafe {
        let _ = EnumWindows(
            Some(enum_proc),
            LPARAM(&mut windows_found as *mut Vec<isize> as isize),
        );
    }

    for raw in windows_found {
        let hwnd = HWND(raw as *mut core::ffi::c_void);
        let (visible, class, title) = unsafe {
            (
                IsWindowVisible(hwnd).as_bool(),
                {
                    let mut buf = [0u16; 64];
                    let n = GetClassNameW(hwnd, &mut buf);
                    String::from_utf16_lossy(&buf[..n.max(0) as usize])
                },
                {
                    let len = GetWindowTextLengthW(hwnd);
                    if len <= 0 {
                        String::new()
                    } else {
                        let mut buf = vec![0u16; len as usize + 1];
                        let n = GetWindowTextW(hwnd, &mut buf);
                        String::from_utf16_lossy(&buf[..n.max(0) as usize])
                    }
                },
            )
        };

        // Chromium-family browser main window, Opera's title always ends in "Opera"
        if visible && class == "Chrome_WidgetWin_1" && title.ends_with("Opera") {
            return Some(hwnd);
        }
    }

    None
}

#[cfg(target_os = "windows")]
fn send_browser_skip_keys(next: bool) -> Result<(), String> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, VIRTUAL_KEY, VK_SHIFT, INPUT, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
        KEYEVENTF_KEYUP,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, SetForegroundWindow};

    let hwnd = find_browser_window()
        .ok_or_else(|| "No browser window found for skip shortcut".to_string())?;

    let previous = unsafe { GetForegroundWindow() };
    unsafe {
        let _ = SetForegroundWindow(hwnd);
    }
    std::thread::sleep(std::time::Duration::from_millis(120));

    let key_char: u16 = if next { b'N' as u16 } else { b'P' as u16 };
    let make_input = |vk: VIRTUAL_KEY, flags: KEYBD_EVENT_FLAGS| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: windows::Win32::UI::Input::KeyboardAndMouse::INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };

    let key = VIRTUAL_KEY(key_char);
    let keyup = KEYBD_EVENT_FLAGS(KEYEVENTF_KEYUP.0);
    let inputs = [
        make_input(VK_SHIFT, KEYBD_EVENT_FLAGS(0)),
        make_input(key, KEYBD_EVENT_FLAGS(0)),
        make_input(key, keyup),
        make_input(VK_SHIFT, keyup),
    ];

    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    std::thread::sleep(std::time::Duration::from_millis(80));

    unsafe {
        if !previous.0.is_null() {
            let _ = SetForegroundWindow(previous);
        }
    }

    if sent != inputs.len() as u32 {
        return Err("Failed to send skip keystrokes to browser".to_string());
    }
    Ok(())
}

async fn send_media_command(command: MediaCommand) -> Result<(), String> {
    tokio::task::spawn_blocking(move || send_smtc_command_blocking(command))
        .await
        .map_err(|e| format!("Task failed: {}", e))?
}

#[tauri::command]
pub async fn media_play_pause() -> Result<(), String> {
    send_media_command(MediaCommand::PlayPause).await
}

#[tauri::command]
pub async fn media_next() -> Result<(), String> {
    send_media_command(MediaCommand::Next).await
}

#[tauri::command]
pub async fn media_previous() -> Result<(), String> {
    send_media_command(MediaCommand::Previous).await
}
