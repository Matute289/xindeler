# Report Bug Button Implementation Plan (Plan C)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a "Report Bug" button to the in-game EscMenu that sends the current session's telemetry log to a configured VPS endpoint via HTTP POST, without blocking the game loop.

**Architecture:** `ureq` (sync HTTP, no tokio) runs in a background thread spawned on button press. The URL is configured in `NetworkingSettings`. The button is a 7th entry below "Quit" in the existing conrod EscMenu widget. A transient HUD notification shows success/failure after the upload completes. The feature is independent of `logging-verbose` — the endpoint can be called even in builds without the verbose feature (the payload will just be empty/absent).

**Prerequisite:** Plan A must be complete so `client_telemetry.jsonl` and `client_err.log` exist to be sent. Plan B adds richer data but is not required for Plan C.

**Tech Stack:** `ureq 2.x` (sync HTTP), `conrod_core` (UI widget), existing `tracing`/`serde_json`.

---

## File Map

| Action | File | Purpose |
|--------|------|---------|
| Modify | `voxygen/Cargo.toml` | Add `ureq` dependency |
| Modify | `voxygen/src/settings/networking.rs` | Add `bug_report_url` field |
| Create | `voxygen/src/bug_report.rs` | `send_bug_report()` — read logs + POST to VPS |
| Modify | `voxygen/src/lib.rs` | Declare `bug_report` module |
| Modify | `voxygen/src/hud/esc_menu.rs` | Add `menu_button_7`, `Event::ReportBug`, expand frame |
| Modify | `voxygen/src/hud/mod.rs` | Handle `Event::ReportBug`, show notification |

---

### Task 1: Add `ureq` dependency

**Files:**
- Modify: `voxygen/Cargo.toml`

- [ ] **Step 1: Locate the `[dependencies]` section**
```bash
grep -n "^\[dependencies\]\|^ureq\|^reqwest\|^serde_json\b" voxygen/Cargo.toml | head -10
```

- [ ] **Step 2: Add `ureq` to `[dependencies]` in `voxygen/Cargo.toml`**

Find the alphabetical insertion point near `u`-prefixed deps. Add:
```toml
ureq = { version = "2", features = ["json"] }
```

- [ ] **Step 3: Verify it resolves**
```bash
source "$HOME/.cargo/env"
cargo check -p veloren-voxygen 2>&1 | grep "^error" | head -5
```
Expected: no errors.

- [ ] **Step 4: Commit**
```bash
git add voxygen/Cargo.toml
git commit -m "feat(report-bug): add ureq HTTP client dep to voxygen"
```

---

### Task 2: Add `bug_report_url` to settings

**Files:**
- Modify: `voxygen/src/settings/networking.rs`

The `NetworkingSettings` struct is at the top of this file. We add `bug_report_url: Option<String>` so the feature is opt-in by default.

- [ ] **Step 1: Open `voxygen/src/settings/networking.rs` and add the field**

Open `voxygen/src/settings/networking.rs`. Find the struct definition:
```rust
pub struct NetworkingSettings {
    pub username: String,
    pub servers: Vec<String>,
    ...
```

Add the field after the last existing field:
```rust
    pub bug_report_url: Option<String>,
```

- [ ] **Step 2: Add default value**

In the `Default` impl (at line ~21), inside `NetworkingSettings { ... }`, add:
```rust
bug_report_url: None,
```

- [ ] **Step 3: Verify compilation**
```bash
cargo check -p veloren-voxygen 2>&1 | grep "^error" | head -5
```

- [ ] **Step 4: Commit**
```bash
git add voxygen/src/settings/networking.rs
git commit -m "feat(report-bug): add bug_report_url to NetworkingSettings"
```

---

### Task 3: Create `bug_report.rs` — sender module

**Files:**
- Create: `voxygen/src/bug_report.rs`

This module exposes one function: `send_bug_report(url, logs_dir)`. It runs in the calling thread (no tokio). Callers should spawn a thread themselves. The function reads up to 500 lines from the most recent telemetry file and 200 lines from the most recent err log, then POSTs a JSON payload.

- [ ] **Step 1: Create `voxygen/src/bug_report.rs`**

```rust
use std::{
    fs,
    io::{self, BufRead},
    path::{Path, PathBuf},
};
use tracing::{error, info};

/// Result returned by send_bug_report — used to update the UI notification.
#[derive(Debug, Clone)]
pub enum BugReportResult {
    Sent,
    Skipped(String), // human-readable reason (no URL configured, no logs, etc.)
    Failed(String),  // error message
}

/// Maximum lines to include from telemetry log
const MAX_TELEMETRY_LINES: usize = 500;
/// Maximum lines to include from error log
const MAX_ERR_LINES: usize = 200;

/// Find the most recently modified file matching a glob-like prefix in a directory.
fn find_latest(dir: &Path, contains: &str) -> Option<PathBuf> {
    let entries = fs::read_dir(dir).ok()?;
    let mut candidates: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .contains(contains)
        })
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            let modified = meta.modified().ok()?;
            Some((modified, e.path()))
        })
        .collect();
    candidates.sort_by(|a, b| b.0.cmp(&a.0));
    candidates.into_iter().next().map(|(_, p)| p)
}

/// Read the last `max_lines` lines from a file (or all lines if fewer).
fn read_tail(path: &Path, max_lines: usize) -> Vec<String> {
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(e) => {
            error!(?e, ?path, "Cannot open log file for bug report");
            return Vec::new();
        }
    };
    let reader = io::BufReader::new(file);
    let lines: Vec<String> = reader.lines().flatten().collect();
    let skip = lines.len().saturating_sub(max_lines);
    lines[skip..].to_vec()
}

/// Send a bug report to the configured URL.
/// This function is synchronous — call from a background thread.
pub fn send_bug_report(url: &str, logs_dir: &Path) -> BugReportResult {
    if url.trim().is_empty() {
        return BugReportResult::Skipped("bug_report_url is empty".to_string());
    }

    // Collect telemetry
    let telemetry_lines = find_latest(logs_dir, "telemetry")
        .map(|p| read_tail(&p, MAX_TELEMETRY_LINES))
        .unwrap_or_default();

    // Collect error log
    let err_lines = find_latest(logs_dir, "client_err")
        .or_else(|| find_latest(logs_dir, "_err"))
        .map(|p| read_tail(&p, MAX_ERR_LINES))
        .unwrap_or_default();

    let payload = serde_json::json!({
        "client_version": env!("CARGO_PKG_VERSION"),
        "platform": std::env::consts::OS,
        "telemetry": telemetry_lines,
        "errors": err_lines,
    });

    info!(url, telemetry_lines = telemetry_lines.len(), err_lines = err_lines.len(), "Sending bug report");

    match ureq::post(url).send_json(payload) {
        Ok(resp) => {
            info!(status = resp.status(), "Bug report sent");
            BugReportResult::Sent
        }
        Err(ureq::Error::Status(code, _)) => {
            let msg = format!("Server returned HTTP {code}");
            error!(code, "Bug report server error");
            BugReportResult::Failed(msg)
        }
        Err(e) => {
            let msg = format!("Network error: {e}");
            error!(?e, "Bug report network error");
            BugReportResult::Failed(msg)
        }
    }
}
```

- [ ] **Step 2: Add `serde_json` to voxygen if not already present**
```bash
grep "^serde_json" voxygen/Cargo.toml
```
If absent, add to `voxygen/Cargo.toml`:
```toml
serde_json = { workspace = true }
```

- [ ] **Step 3: Verify `serde_json` is in the workspace**
```bash
grep "^serde_json" Cargo.toml
```
If absent (unlikely), add to workspace `[dependencies]`:
```toml
serde_json = "1"
```

- [ ] **Step 4: Check compilation**
```bash
cargo check -p veloren-voxygen 2>&1 | grep "^error" | head -10
```
Note: `bug_report.rs` won't compile yet until declared in `lib.rs` (Task 4).

- [ ] **Step 5: Commit**
```bash
git add voxygen/src/bug_report.rs voxygen/Cargo.toml
git commit -m "feat(report-bug): add bug_report module with send_bug_report()"
```

---

### Task 4: Declare `bug_report` module in voxygen

**Files:**
- Modify: `voxygen/src/lib.rs`

- [ ] **Step 1: Find the module declarations in `voxygen/src/lib.rs`**
```bash
grep -n "^mod \|^pub mod " voxygen/src/lib.rs | head -20
```

- [ ] **Step 2: Add module declaration**

In `voxygen/src/lib.rs`, find the alphabetical position for `b` modules and add:
```rust
mod bug_report;
```

- [ ] **Step 3: Verify compilation**
```bash
cargo check -p veloren-voxygen 2>&1 | grep "^error" | head -10
```
Expected: no errors.

- [ ] **Step 4: Commit**
```bash
git add voxygen/src/lib.rs
git commit -m "feat(report-bug): declare bug_report module"
```

---

### Task 5: Add `ReportBug` event and `menu_button_7` to EscMenu

**Files:**
- Modify: `voxygen/src/hud/esc_menu.rs`

Current state:
- Frame is `w_h(240.0, 380.0)` at line 75
- 6 buttons: menu_button_1 through menu_button_6
- `Event` enum has `OpenSettings`, `CharacterSelection`, `Logout`, `Quit`, `Close`

We add a 7th button "Report Bug" between "Characters" and "Logout", shifting Logout and Quit down. The frame needs ~55px more height.

- [ ] **Step 1: Add `menu_button_7` to `widget_ids!`**

In `voxygen/src/hud/esc_menu.rs`, find the `widget_ids!` block and add `menu_button_7`:
```rust
widget_ids! {
    struct Ids {
        esc_bg,
        banner_top,
        menu_button_1,
        menu_button_2,
        menu_button_3,
        menu_button_4,
        menu_button_5,
        menu_button_6,
        menu_button_7,
    }
}
```

- [ ] **Step 2: Add `ReportBug` to `Event` enum**

Find the `pub enum Event` block and add:
```rust
pub enum Event {
    OpenSettings(SettingsTab),
    CharacterSelection,
    ReportBug,
    Logout,
    Quit,
    Close,
}
```

- [ ] **Step 3: Expand the frame height**

Change line 75 from:
```rust
            .w_h(240.0, 380.0)
```
to:
```rust
            .w_h(240.0, 440.0)
```
(adds ~60px to accommodate the 7th button)

- [ ] **Step 4: Add the "Report Bug" button after the "Characters" button**

The current button chain is:
- menu_button_1 (Resume)
- menu_button_2 (Settings, margin -65)
- menu_button_3 (Controls, margin -55)
- menu_button_4 (Characters, margin -55)
- menu_button_5 (Logout, margin -65)
- menu_button_6 (Quit, margin -55)

Insert menu_button_7 ("Report Bug") between Characters and Logout. Change the existing Logout button to anchor on menu_button_7, and Quit stays anchored on menu_button_5 (which is now Logout):

After the Characters block (currently ending at line ~148), add:
```rust
        // Report Bug
        if Button::image(self.imgs.button)
            .mid_bottom_with_margin_on(state.ids.menu_button_4, -65.0)
            .w_h(210.0, 50.0)
            .hover_image(self.imgs.button_hover)
            .press_image(self.imgs.button_press)
            .label("Report Bug")
            .label_y(conrod_core::position::Relative::Scalar(3.0))
            .label_color(TEXT_COLOR)
            .label_font_size(self.fonts.cyri.scale(20))
            .label_font_id(self.fonts.cyri.conrod_id)
            .set(state.ids.menu_button_7, ui)
            .was_clicked()
        {
            return Some(Event::ReportBug);
        };
```

Then update the Logout button (currently menu_button_5) to anchor on menu_button_7 instead of menu_button_4:
```rust
        // Logout
        if Button::image(self.imgs.button)
            .mid_bottom_with_margin_on(state.ids.menu_button_7, -55.0)   // was menu_button_4
            .w_h(210.0, 50.0)
```

The Quit button (menu_button_6) stays anchored on menu_button_5 — no change needed there.

Note: "Report Bug" uses a hardcoded English label for now (i18n key can be added later). This avoids needing to add a new i18n key for an infrastructure-only feature.

- [ ] **Step 5: Verify compilation**
```bash
cargo check -p veloren-voxygen 2>&1 | grep "^error" | head -10
```

- [ ] **Step 6: Commit**
```bash
git add voxygen/src/hud/esc_menu.rs
git commit -m "feat(report-bug): add Report Bug button to EscMenu"
```

---

### Task 6: Handle `Event::ReportBug` in HUD mod

**Files:**
- Modify: `voxygen/src/hud/mod.rs`

The esc_menu event handler is at lines 3997–4025. We handle `Event::ReportBug` by spawning a background thread that calls `crate::bug_report::send_bug_report`. We use the existing notification system to show a result. We also need the logs directory and the configured URL.

- [ ] **Step 1: Find how settings and logs_dir are available in the HUD**
```bash
grep -n "global_state\|settings\b\|logs_dir\|userdata" voxygen/src/hud/mod.rs | grep -v "//\|fmt\|test" | head -20
grep -n "pub struct Hud\b\|GlobalState\|fn maintain\b" voxygen/src/hud/mod.rs | head -10
```

- [ ] **Step 2: Determine Hud struct fields and how to pass URL + logs_dir**

The HUD `maintain` function receives `global_state` which contains settings. Check:
```bash
grep -n "fn maintain\|global_state" voxygen/src/hud/mod.rs | head -10
```

The `global_state.settings.networking.bug_report_url` is available. For `logs_dir`, it's computed at startup in `main.rs` — either pass it to `Hud::maintain` or use `common_base::userdata_dir().join("voxygen").join("logs")` inline (same path computed in Plan A Task 6).

Use inline path computation in the handler to avoid modifying function signatures:
```rust
let logs_dir = common_base::userdata_dir().join("voxygen").join("logs");
```

- [ ] **Step 3: Add import to the top of `voxygen/src/hud/mod.rs`**

Near the top, add if not already present:
```rust
use std::sync::{Arc, Mutex};
```

- [ ] **Step 4: Add notification state for in-progress and result**

Find where `Hud` struct fields are defined. Add a field to track the background report thread:
```bash
grep -n "^pub struct Hud\b" voxygen/src/hud/mod.rs
```

In the `Hud` struct, add:
```rust
bug_report_status: Option<Arc<Mutex<Option<crate::bug_report::BugReportResult>>>>,
```

In the `Hud::new(...)` initialization, initialize it:
```rust
bug_report_status: None,
```

- [ ] **Step 5: Handle `Event::ReportBug` in the event match block**

Find the match arm at line ~4018:
```rust
Some(esc_menu::Event::Quit) => events.push(Event::Quit),
```

Add before or after `Quit`:
```rust
Some(esc_menu::Event::ReportBug) => {
    let url = global_state
        .settings
        .networking
        .bug_report_url
        .clone()
        .unwrap_or_default();

    if url.is_empty() {
        warn!("bug_report_url not configured — skipping bug report");
        // Show a toast-style notification (reuse existing notification system)
        self.new_message(comp::ChatType::CommandError, "Bug report URL not configured.".to_string());
    } else {
        let logs_dir = common_base::userdata_dir()
            .join("voxygen")
            .join("logs");
        let status_slot: Arc<Mutex<Option<crate::bug_report::BugReportResult>>> =
            Arc::new(Mutex::new(None));
        let status_clone = Arc::clone(&status_slot);
        self.bug_report_status = Some(status_slot);
        std::thread::spawn(move || {
            let result = crate::bug_report::send_bug_report(&url, &logs_dir);
            if let Ok(mut guard) = status_clone.lock() {
                *guard = Some(result);
            }
        });
        info!("Bug report thread spawned");
        self.new_message(comp::ChatType::CommandInfo, "Bug report sending…".to_string());
        self.show.esc_menu = false;
    }
},
```

Note: `self.new_message` may have a different signature — adjust to match the existing chat message API. Search with:
```bash
grep -n "fn new_message\|CommandInfo\|CommandError\|push.*chat" voxygen/src/hud/mod.rs | head -10
```

If `new_message` doesn't exist, find the equivalent function used for other command results and use that pattern.

- [ ] **Step 6: Poll the background result each frame**

Find the `maintain` function body (called each frame). Add a check that reads the result and clears it:

```bash
grep -n "fn maintain\b" voxygen/src/hud/mod.rs | head -3
```

Near the top of `maintain()`, before widget construction, add:
```rust
// Poll bug report result
if let Some(status_arc) = &self.bug_report_status {
    if let Ok(mut guard) = status_arc.try_lock() {
        if let Some(result) = guard.take() {
            let msg = match result {
                crate::bug_report::BugReportResult::Sent => "Bug report sent. Thank you!".to_string(),
                crate::bug_report::BugReportResult::Skipped(r) => format!("Bug report skipped: {r}"),
                crate::bug_report::BugReportResult::Failed(e) => format!("Bug report failed: {e}"),
            };
            self.new_message(comp::ChatType::CommandInfo, msg);
            self.bug_report_status = None;
        }
    }
}
```

- [ ] **Step 7: Verify compilation**
```bash
cargo check -p veloren-voxygen 2>&1 | grep "^error" | head -20
```

Fix any type mismatches. Common issues:
- `comp::ChatType` path — may need `common::comp::ChatType`
- `self.new_message` signature — adapt to whatever the HUD uses for system messages
- `common_base::userdata_dir()` path — verify with `grep -rn "userdata_dir" common-base/`

- [ ] **Step 8: Commit**
```bash
git add voxygen/src/hud/mod.rs
git commit -m "feat(report-bug): handle ReportBug event, spawn sender thread, show result"
```

---

### Task 7: Integration test

- [ ] **Step 1: Build the game**
```bash
source "$HOME/.cargo/env"
cargo build --bin veloren-voxygen 2>&1 | tail -5
```
Expected: `Finished dev profile`.

- [ ] **Step 2: Launch and test UI (singleplayer)**

Launch the game and enter singleplayer:
```bash
cargo run --bin veloren-voxygen
```

Open EscMenu (Esc key). Verify:
1. 7 buttons are visible (the new "Report Bug" button appears between "Characters" and "Logout")
2. The frame fits all 7 buttons without overlap or clipping
3. Clicking "Report Bug" with no `bug_report_url` configured shows "Bug report URL not configured." in chat
4. Clicking "Resume" still works
5. Clicking "Logout" and "Quit" still work

- [ ] **Step 3: Test with a configured URL**

In `userdata/voxygen/settings.ron` (created by the game on first run), add the `bug_report_url` field to the `networking` section:
```ron
networking: (
    ...
    bug_report_url: Some("http://localhost:9999/bug-report"),
    ...
)
```

Start a minimal HTTP listener:
```bash
python3 -m http.server 9999 &
```

Relaunch the game and click "Report Bug". Verify:
1. "Bug report sending…" appears in chat
2. Within a few seconds, "Bug report failed: ..." or "Bug report sent." appears
3. The server receives a POST request (visible in python http.server output)

Kill the python server:
```bash
kill %1
```

- [ ] **Step 4: Verify no regression in frame height**

Open EscMenu and visually check:
- Resume button is at the top, fully visible
- Quit button is at the bottom, not clipped by the frame edge
- Report Bug button is between Characters and Logout

- [ ] **Step 5: Run clippy**
```bash
cargo clippy -p veloren-voxygen -- -D warnings 2>&1 | grep "^error\|^warning.*error" | head -10
```

- [ ] **Step 6: Final commit and push**
```bash
git push origin main
```

---

## Notes for implementer

- **No tokio**: `ureq` is fully synchronous. The `std::thread::spawn` approach is correct. Do not add `tokio` or `async` anywhere in this plan.
- **`new_message` may not exist**: If the HUD doesn't have a `new_message` method, look for how the game currently shows "You gained X XP" style messages and replicate that pattern.
- **Frame height**: If 440px is too large or small, adjust visually. The original 380px fits 6 buttons; each button + margin is ~55–65px, so +60px should fit 7 comfortably.
- **i18n**: The "Report Bug" label is hardcoded English for now. Adding an i18n key is a separate, optional improvement.
- **VPS endpoint**: The plan doesn't implement the server side. The VPS endpoint should accept POST JSON with fields `client_version`, `platform`, `telemetry[]`, `errors[]`. A simple Node.js or Python script can receive and write to disk.
- **Security**: The telemetry payload may contain position coordinates and player names. The URL should be HTTPS in production to prevent interception. `ureq` validates TLS by default.
