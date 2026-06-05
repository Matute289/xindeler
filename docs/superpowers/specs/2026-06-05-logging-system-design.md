# Logging System Design

## Goal

Implement a structured, multi-sink logging system for Veloren with four log files (client + server × info + err), log rotation and retention policies, comprehensive in-code coverage, and an in-game "Report Bug" button that uploads client logs to a VPS endpoint.

## Architecture Overview

Three independent phases, each testeable in isolation:

- **Phase 1 — Infrastructure**: 4-file logging system + feature flag
- **Phase 2 — Coverage**: Add `info!`/`warn!`/`debug!`/`error!` calls throughout client and server code
- **Phase 3 — Report Bug**: In-game button + HTTP POST to VPS endpoint

```
[code]                [tracing layers]              [files]
info!(...)  ──┬──▶  BoundedWriter(INFO filter) ──▶  client_info.log  (logging-verbose only)
warn!(...)  ──┤
error!(...) ──┴──▶  BoundedWriter(WARN filter) ──▶  client_err.log   (always)
```

## Phase 1 — Infrastructure

### Feature Flag

`logging-verbose` controls whether the INFO/DEBUG sink is active. Without it, only WARN+ERROR is captured to file.

```toml
# common/frontend/Cargo.toml
[features]
logging-verbose = []

# voxygen/Cargo.toml and server-cli/Cargo.toml
[features]
logging-verbose = ["common-frontend/logging-verbose"]
```

Dev builds include `--features logging-verbose`. Release builds omit it — only error logs are written.

### Log Files

```
userdata/voxygen/logs/
  2026-06-05_14h_client_info.log        ← DEBUG+INFO  (logging-verbose only)
  2026-06-05_14h_client_info.2.log      ← rotated by line count
  2026-06-05_client_err.log             ← WARN+ERROR  (always)

userdata/server/logs/
  2026-06-05_14h_server_info.log        ← DEBUG+INFO  (logging-verbose only)
  2026-06-05_server_err.log             ← WARN+ERROR  (always)
```

Rotated files are compressed: `2026-06-05_14h_client_info.1.log.gz`.

### `init_split_logs()` — New function in `common/frontend/src/lib.rs`

```rust
/// Initialise split logging: one err sink (always) and one info sink (logging-verbose feature).
/// `prefix` is "client" or "server". `logs_dir` is the directory to write into.
/// Returns guards that flush logs on drop.
pub fn init_split_logs(prefix: &str, logs_dir: &Path) -> (Vec<impl Drop>, Arc<AtomicBool>)
```

Returns a `has_errors: Arc<AtomicBool>` that is set to `true` whenever a WARN or ERROR event is emitted. The caller holds this to determine whether to clean up info logs on exit. Internally, `init_split_logs` registers a custom `tracing::Layer` (`ErrorDetectorLayer`) whose `on_event` sets the flag when the event's level is WARN or ERROR.

The existing `init()` and `init_stdout()` functions are left untouched. All existing call sites continue to work.

### Call Sites

**`voxygen/src/main.rs`** — replace `init_stdout(Some(...))`:
```rust
let (log_dir) = userdata_dir.join("voxygen").join("logs");
let (_guards, has_errors) = common_frontend::init_split_logs("client", &log_dir);
```

**`server-cli/src/main.rs`** — add alongside existing init:
```rust
let log_dir = common_base::userdata_dir().join("server").join("logs");
let (_guards, _has_errors) = common_frontend::init_split_logs("server", &log_dir);
```

### Rotation Policy

| Log | Time rotation | Size rotation | Compression |
|-----|--------------|--------------|-------------|
| `*_info.log` (client) | Hourly | 5,000 lines | gzip on rotate |
| `*_err.log` (client) | Daily | 1,000 lines | gzip on rotate |
| `*_info.log` (server) | Hourly | 10,000 lines | gzip on rotate |
| `*_err.log` (server) | Daily | 1,000 lines | gzip on rotate |

**`BoundedWriter`** — a `MakeWriter` wrapper that wraps `tracing_appender::rolling::hourly()` or `daily()` and additionally rotates when a line-count limit is reached. Uses `AtomicU64` for the counter. When the limit is reached it creates a new file with an incrementing numeric suffix.

Compression of rotated files runs in a background thread via a `std::sync::mpsc::channel`. When `BoundedWriter` closes a file, it sends the path to the compression thread which gzips it with `flate2`.

New dependency in `common/frontend/Cargo.toml`:
```toml
flate2 = "1.0"
```

### Retention Policy

**Client:**
- `client_info.log`: retained 24 hours. Deleted on startup if older than 24h.
- `client_info.log`: **also deleted on clean exit** if `has_errors == false` (no WARN/ERROR occurred during the session).
- `client_err.log`: retained 7 days.

**Server:**
- `server_info.log`: retained 30 days.
- `server_err.log`: retained 30 days.

**`LogLifecycleManager`** — runs cleanup logic:
- On startup: scan logs dir, delete files beyond retention threshold.
- Server: additionally spawns a background thread that re-runs cleanup every 24 hours.
- Clean-exit hook (client): called from voxygen's shutdown path. Deletes today's info log files if `has_errors == false`.

## Phase 2 — Comprehensive Log Coverage

### Log Level Taxonomy

| Level | Sink | Use for |
|-------|------|---------|
| `error!` | `*_err.log` | Unrecoverable failures: panics, persistence errors, network hard-fail |
| `warn!` | `*_err.log` | Recoverable problems: asset load retry, slow tick, desync |
| `info!` | `*_info.log` | Significant game events: player join/leave, death, trade, site load |
| `debug!` | `*_info.log` | Frequent events: combat hits, inventory changes, chunk load/unload |

### Server-Side Coverage

**`server/src/lib.rs`**
- `info!` on player connect: `info!(player = %alias, entity = ?uid, "Player connected")`
- `info!` on player disconnect: reason, session duration
- `warn!` on slow tick: `warn!(tick_ms, "Slow server tick")`

**`server/src/events/entity_manipulation.rs`**
- `info!` on player death: player name, cause, position
- `debug!` on entity damage: attacker, target, amount

**`server/src/persistence/`**
- `error!` on save failure: entity uid, error
- `error!` on load failure: character id, error
- `info!` on successful character save

**`server/src/sys/`** (tick systems)
- `debug!` on entity count per tick (every 100 ticks to avoid spam)

### Client-Side Coverage

**`client/src/lib.rs`**
- `error!` on network error / disconnect
- `info!` on successful server connection: server address, ping
- `warn!` on message decode error

**`voxygen/src/audio/`**
- `warn!` on audio device init failure
- `error!` on audio file load failure

**`voxygen/src/render/`** / `voxygen/src/scene/`
- `error!` on shader compile failure
- `warn!` on missing texture / model
- `debug!` on frame time > 33ms (< 30 fps)

**`common-systems/src/`**
- `debug!` on combat hit: attacker, target, damage, skill
- `warn!` on physics anomaly (entity outside world bounds)

**`voxygen/src/hud/chat.rs`** (or equivalent)
- `debug!` on player chat message sent

### Log Format

Structured fields with tracing:
```rust
info!(player = %alias, pos = ?position, cause = ?damage_kind, "Player died");
warn!(tick_ms = duration.as_millis(), entity_count, "Slow tick");
debug!(attacker = ?attacker_uid, target = ?target_uid, damage, "Combat hit");
```

## Phase 3 — Report Bug Button

### UI

New button `menu_button_7` added to `voxygen/src/hud/esc_menu.rs`.

On click → opens a confirmation dialog (`prompt_dialog.rs` pattern):
> "Send your session logs to the server? This helps diagnose bugs. No personal data beyond gameplay events is included."

Options: **Send** / **Cancel**

On confirm → shows a "Sending…" spinner in the dialog → result toast notification.

### Send Flow

```
[user clicks Send]
  → read client_err.log + client_info.log (if exists) from logs_dir
  → spawn std::thread (non-blocking)
      → HTTP POST JSON to bug_report_url
      → send result back via std::sync::mpsc channel
  → game loop polls channel → shows toast "Report sent ✓" or "Failed to send report"
```

### HTTP Payload

```json
{
  "game_version": "0.17.0-dev",
  "platform": "macos",
  "timestamp": "2026-06-05T15:30:00Z",
  "client_err_log": "<file contents or null>",
  "client_info_log": "<file contents or null>"
}
```

> Note: `client_info_log` may contain chat messages (logged at DEBUG level). This is intentional — chat context helps diagnose bugs. No passwords or auth tokens are ever logged.
```

### HTTP Client

New dependency in `voxygen/Cargo.toml`:
```toml
ureq = { version = "2", features = ["json"] }
```

`ureq` is a sync HTTP client (~30KB, no tokio dependency). Used inside the spawned thread to avoid introducing async dependencies into voxygen's bug-report path.

### Configuration

New field in `voxygen/src/settings.rs`:
```rust
pub bug_report_url: String,  // default: "http://<VPS_IP>/bug-report"
```

Serialized to `userdata/client/settings.ron`. The default URL points to the VPS endpoint. Users can override in their settings file.

### VPS Endpoint

A lightweight HTTP receiver on the VPS that writes incoming JSON payloads to `~/bug-reports/TIMESTAMP.json`. Implementation is **out of scope** for this plan — documented separately in `~/MyServerVPS`. The endpoint URL must be reachable before Phase 3 can be fully tested.

## Dependencies Added

| Crate | Version | Added to | Purpose |
|-------|---------|----------|---------|
| `flate2` | `1.0` | `common/frontend` | gzip compression of rotated logs |
| `ureq` | `2` | `voxygen` | HTTP POST for bug reports |

`tracing-appender` is already a dependency of `common/frontend` and provides the rolling writers.

## Testing

**Phase 1:**
- Build with and without `logging-verbose` — verify correct files are created/absent
- Run client, trigger an error, close cleanly — verify err log kept, info log deleted
- Run client with no errors, close — verify info log deleted
- Run server 25+ hours — verify hourly rotation produces multiple files, files >24h deleted

**Phase 2:**
- Trigger each logged event in-game and grep the corresponding log file
- Slow tick: artificially pause the server tick thread and verify `warn!` appears

**Phase 3:**
- Click Report Bug, confirm — verify POST received at VPS endpoint
- Disconnect VPS — verify failure toast shown, game continues normally
- Verify no PII (passwords, tokens) appears in log payload

## Out of Scope

- Server log upload (server logs checked manually on VPS)
- Log viewer UI inside the game
- Log encryption
- Automatic crash reporting without user action
