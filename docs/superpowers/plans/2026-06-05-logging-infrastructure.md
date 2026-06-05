# Logging Infrastructure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace Veloren's single-file logging with a 6-sink system: `{prefix}_err.log` (always), `{prefix}_info.log` (dev), `{prefix}_telemetry.jsonl` (dev, Claude-only JSON Lines), plus hourly/daily rotation, line-count rotation, gzip compression, and lifecycle cleanup.

**Architecture:** A new `init_split_logs(prefix, logs_dir)` function in `common/frontend` replaces the existing `init_stdout()` call site in voxygen and adds file logging to server-cli. Three helper structs implement the new sinks: `BoundedMakeWriter` (file writer with time+size rotation and gzip), `TelemetryLayer` (JSON Lines tracing layer for target="telemetry" events), and `LogLifecycleManager` (startup cleanup + clean-exit hook). In singleplayer mode, server and client code share one process — all telemetry events from all crates land in a single `client_telemetry.jsonl`, giving unified visibility into both sides without extra work.

**Tech Stack:** Rust nightly, `tracing` + `tracing-subscriber` + `tracing-appender` (already present), `flate2` (new), `chrono` (already present in workspace).

---

## File Map

| Action | File | Purpose |
|--------|------|---------|
| Modify | `common/frontend/Cargo.toml` | Add `logging-verbose` feature, `flate2` dep |
| Create | `common/frontend/src/bounded_writer.rs` | `BoundedMakeWriter` — time+size rotation + gzip |
| Create | `common/frontend/src/telemetry_layer.rs` | `TelemetryLayer` — JSON Lines sink for telemetry events |
| Create | `common/frontend/src/lifecycle.rs` | `ErrorDetectorLayer`, `LogLifecycleManager`, `LogGuards` |
| Modify | `common/frontend/src/lib.rs` | Add `init_split_logs()`, pub re-exports |
| Modify | `voxygen/Cargo.toml` | Add `logging-verbose` feature |
| Modify | `voxygen/src/main.rs` | Replace `init_stdout(Some(...))` with `init_split_logs` |
| Modify | `server-cli/Cargo.toml` | Add `logging-verbose` feature |
| Modify | `server-cli/src/main.rs` | Add `init_split_logs` for basic mode |

---

### Task 1: Feature flags and `flate2` dependency

**Files:**
- Modify: `common/frontend/Cargo.toml`
- Modify: `voxygen/Cargo.toml`
- Modify: `server-cli/Cargo.toml`

- [ ] **Step 1: Add `logging-verbose` feature and `flate2` to `common/frontend/Cargo.toml`**

The file currently has:
```toml
[features]
tracy = ["common-base/tracy", "tracing-tracy"]
```

Replace with:
```toml
[features]
tracy = ["common-base/tracy", "tracing-tracy"]
logging-verbose = []
```

Then in `[dependencies]`, add after the `# Logging` comment:
```toml
flate2 = "1.0"
chrono = { workspace = true }
```

`chrono` is already in the workspace (confirmed in root `Cargo.toml`). Use `chrono = { workspace = true }`.

- [ ] **Step 2: Propagate `logging-verbose` to `voxygen/Cargo.toml`**

In `voxygen/Cargo.toml`, the `[features]` section starts at line 25. Add the new feature alongside the existing ones:
```toml
logging-verbose = ["common-frontend/logging-verbose"]
```

- [ ] **Step 3: Propagate `logging-verbose` to `server-cli/Cargo.toml`**

In `server-cli/Cargo.toml`, the `[features]` section is at line 20. Add:
```toml
logging-verbose = ["common-frontend/logging-verbose"]
```

- [ ] **Step 4: Verify `flate2` resolves**
```bash
source "$HOME/.cargo/env"
cargo check -p veloren-common-frontend 2>&1 | grep -E "error|warning" | head -20
```
Expected: no errors about missing `flate2`.

- [ ] **Step 5: Commit**
```bash
git add common/frontend/Cargo.toml voxygen/Cargo.toml server-cli/Cargo.toml
git commit -m "feat(logging): add logging-verbose feature flag + flate2 dep"
```

---

### Task 2: `BoundedMakeWriter` — rotating file writer with gzip

**Files:**
- Create: `common/frontend/src/bounded_writer.rs`

This is the core component: a `MakeWriter` implementation that writes to a file, rotates when a time bucket changes (hourly or daily) OR when line count exceeds the limit, and queues old files for gzip compression via a channel.

- [ ] **Step 1: Create `common/frontend/src/bounded_writer.rs`**

```rust
use chrono::Utc;
use flate2::{Compression, write::GzEncoder};
use std::{
    fs::{self, File},
    io::{self, BufWriter, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    thread,
};
use tracing_appender::non_blocking::WorkerGuard as AppenderGuard;
use tracing_subscriber::fmt::MakeWriter;

pub enum Rotation {
    Hourly,
    Daily,
}

struct WriterState {
    writer: BufWriter<File>,
    path: PathBuf,
    line_count: u64,
    seq: u32,
    bucket: String,
}

pub struct BoundedMakeWriter {
    state: Arc<Mutex<WriterState>>,
    base_dir: PathBuf,
    prefix: String,
    rotation: Rotation,
    max_lines: u64,
    compress_tx: mpsc::SyncSender<PathBuf>,
}

/// Held by the caller to keep the compression thread alive.
pub struct CompressionGuard(Option<thread::JoinHandle<()>>);

impl Drop for CompressionGuard {
    fn drop(&mut self) {
        // join is best-effort; if the thread panicked, ignore
        if let Some(h) = self.0.take() { let _ = h.join(); }
    }
}

impl BoundedMakeWriter {
    pub fn new(
        base_dir: &Path,
        prefix: &str,
        rotation: Rotation,
        max_lines: u64,
    ) -> (Self, CompressionGuard) {
        let (tx, rx) = mpsc::sync_channel::<PathBuf>(64);
        let compress_thread = thread::Builder::new()
            .name(format!("log-compress-{prefix}"))
            .spawn(move || {
                for path in rx {
                    if let Err(e) = compress_file(&path) {
                        eprintln!("[log] compress failed for {}: {e}", path.display());
                    }
                }
            })
            .expect("spawn compress thread");

        let bucket = current_bucket(&rotation);
        let (path, writer) = open_log_file(base_dir, prefix, &bucket, 1);
        let state = Arc::new(Mutex::new(WriterState {
            writer,
            path,
            line_count: 0,
            seq: 1,
            bucket,
        }));
        (
            Self {
                state,
                base_dir: base_dir.to_owned(),
                prefix: prefix.to_owned(),
                rotation,
                max_lines,
                compress_tx: tx,
            },
            CompressionGuard(Some(compress_thread)),
        )
    }
}

fn current_bucket(r: &Rotation) -> String {
    let now = Utc::now();
    match r {
        Rotation::Hourly => now.format("%Y-%m-%d_%Hh").to_string(),
        Rotation::Daily  => now.format("%Y-%m-%d").to_string(),
    }
}

fn open_log_file(dir: &Path, prefix: &str, bucket: &str, seq: u32) -> (PathBuf, BufWriter<File>) {
    let _ = fs::create_dir_all(dir);
    let name = if seq == 1 {
        format!("{bucket}_{prefix}.log")
    } else {
        format!("{bucket}_{prefix}.{seq}.log")
    };
    let path = dir.join(&name);
    let file = File::create(&path).unwrap_or_else(|e| panic!("cannot create log {}: {e}", path.display()));
    (path, BufWriter::new(file))
}

fn compress_file(path: &Path) -> io::Result<()> {
    let gz_path = path.with_extension("log.gz");
    let data = fs::read(path)?;
    let gz_file = File::create(&gz_path)?;
    let mut enc = GzEncoder::new(gz_file, Compression::default());
    enc.write_all(&data)?;
    enc.finish()?;
    fs::remove_file(path)?;
    Ok(())
}

pub struct BoundedWriter<'a> {
    state: std::sync::MutexGuard<'a, WriterState>,
    base_dir: &'a Path,
    prefix: &'a str,
    rotation: &'a Rotation,
    max_lines: u64,
    compress_tx: &'a mpsc::SyncSender<PathBuf>,
}

impl Write for BoundedWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.state.writer.write(buf)?;
        self.state.line_count += buf[..n].iter().filter(|&&b| b == b'\n').count() as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.state.writer.flush()
    }
}

impl Drop for BoundedWriter<'_> {
    fn drop(&mut self) {
        let new_bucket = current_bucket(self.rotation);
        let needs_time_rotate = new_bucket != self.state.bucket;
        let needs_size_rotate = self.state.line_count >= self.max_lines;

        if needs_time_rotate || needs_size_rotate {
            let _ = self.state.writer.flush();
            let old_path = self.state.path.clone();
            // queue old file for gzip; non-blocking (sync_channel with capacity)
            let _ = self.compress_tx.try_send(old_path);

            let (seq, bucket) = if needs_time_rotate {
                (1u32, new_bucket)
            } else {
                (self.state.seq + 1, self.state.bucket.clone())
            };
            let (path, writer) = open_log_file(self.base_dir, self.prefix, &bucket, seq);
            self.state.writer = writer;
            self.state.path = path;
            self.state.line_count = 0;
            self.state.seq = seq;
            self.state.bucket = bucket;
        }
    }
}

impl<'a> MakeWriter<'a> for BoundedMakeWriter {
    type Writer = BoundedWriter<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        BoundedWriter {
            state: self.state.lock().unwrap(),
            base_dir: &self.base_dir,
            prefix: &self.prefix,
            rotation: &self.rotation,
            max_lines: self.max_lines,
            compress_tx: &self.compress_tx,
        }
    }
}
```

- [ ] **Step 2: Add module declaration to `common/frontend/src/lib.rs`**

At the top of the file, after the existing `use` statements, add:
```rust
mod bounded_writer;
pub use bounded_writer::{BoundedMakeWriter, CompressionGuard, Rotation};
```

- [ ] **Step 3: Check it compiles**
```bash
source "$HOME/.cargo/env"
cargo check -p veloren-common-frontend 2>&1 | grep -E "^error" | head -20
```
Expected: no errors.

- [ ] **Step 4: Commit**
```bash
git add common/frontend/src/bounded_writer.rs common/frontend/src/lib.rs
git commit -m "feat(logging): add BoundedMakeWriter with time+size rotation and gzip"
```

---

### Task 3: `ErrorDetectorLayer` and `LogLifecycleManager`

**Files:**
- Create: `common/frontend/src/lifecycle.rs`

`ErrorDetectorLayer` is a tracing `Layer` that sets an `AtomicBool` when a WARN or ERROR event fires. `LogLifecycleManager` runs retention cleanup on startup and optionally deletes info/telemetry logs on clean exit.

- [ ] **Step 1: Create `common/frontend/src/lifecycle.rs`**

```rust
use std::{
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime},
};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;

// ── ErrorDetectorLayer ─────────────────────────────────────────────────────

/// Sets `has_errors` to true whenever a WARN or ERROR event is emitted.
pub struct ErrorDetectorLayer {
    pub has_errors: Arc<AtomicBool>,
}

impl ErrorDetectorLayer {
    pub fn new() -> (Self, Arc<AtomicBool>) {
        let flag = Arc::new(AtomicBool::new(false));
        (Self { has_errors: Arc::clone(&flag) }, flag)
    }
}

impl<S: Subscriber> Layer<S> for ErrorDetectorLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        if *event.metadata().level() <= Level::WARN {
            self.has_errors.store(true, Ordering::Relaxed);
        }
    }
}

// ── LogLifecycleManager ────────────────────────────────────────────────────

pub struct LogLifecycleManager {
    logs_dir: PathBuf,
}

impl LogLifecycleManager {
    pub fn new(logs_dir: PathBuf) -> Self { Self { logs_dir } }

    /// Run on process start: delete files older than their retention window.
    /// - *_info.log, *_telemetry.jsonl → 24 hours (client) / 30 days (server)
    /// - *_err.log → 7 days (client) / 30 days (server)
    pub fn cleanup_on_startup(&self, info_retention: Duration, err_retention: Duration) {
        let now = SystemTime::now();
        let Ok(entries) = fs::read_dir(&self.logs_dir) else { return };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            let Ok(modified) = meta.modified() else { continue };
            let age = now.duration_since(modified).unwrap_or_default();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let expired = if name.contains("_info") || name.contains("_telemetry") {
                age > info_retention
            } else if name.contains("_err") {
                age > err_retention
            } else {
                false
            };
            if expired {
                let _ = fs::remove_file(entry.path());
            }
        }
    }

    /// Run on clean exit: if no errors occurred during the session, delete
    /// info and telemetry log files (they add no diagnostic value).
    pub fn cleanup_on_exit(&self, has_errors: &AtomicBool) {
        if has_errors.load(Ordering::Relaxed) {
            return; // errors occurred — keep all logs
        }
        let Ok(entries) = fs::read_dir(&self.logs_dir) else { return };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.contains("_info") || name.contains("_telemetry") {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}
```

- [ ] **Step 2: Add module to `common/frontend/src/lib.rs`**

Add after the `bounded_writer` module declaration:
```rust
mod lifecycle;
pub use lifecycle::{ErrorDetectorLayer, LogLifecycleManager};
```

- [ ] **Step 3: Check it compiles**
```bash
cargo check -p veloren-common-frontend 2>&1 | grep "^error" | head -20
```

- [ ] **Step 4: Commit**
```bash
git add common/frontend/src/lifecycle.rs common/frontend/src/lib.rs
git commit -m "feat(logging): add ErrorDetectorLayer and LogLifecycleManager"
```

---

### Task 4: `TelemetryLayer` — JSON Lines sink

**Files:**
- Create: `common/frontend/src/telemetry_layer.rs`

Intercepts `trace!` events where `target == "telemetry"` and writes them as compact JSON Lines to a dedicated `.jsonl` file. Used exclusively by Claude for game state analysis; not human-facing.

- [ ] **Step 1: Create `common/frontend/src/telemetry_layer.rs`**

```rust
use chrono::Utc;
use std::{
    fs::{self, File},
    io::{BufWriter, Write},
    path::Path,
    sync::{Arc, Mutex},
};
use tracing::{Event, Subscriber};
use tracing::field::{Field, Visit};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;

pub struct TelemetryLayer {
    writer: Arc<Mutex<BufWriter<File>>>,
}

impl TelemetryLayer {
    /// Returns `None` if the file cannot be created (logs a warning, doesn't panic).
    pub fn new(logs_dir: &Path, prefix: &str) -> Option<Self> {
        let _ = fs::create_dir_all(logs_dir);
        let bucket = Utc::now().format("%Y-%m-%d_%Hh").to_string();
        let name = format!("{bucket}_{prefix}_telemetry.jsonl");
        match File::create(logs_dir.join(&name)) {
            Ok(f) => Some(Self {
                writer: Arc::new(Mutex::new(BufWriter::new(f))),
            }),
            Err(e) => {
                eprintln!("[log] Failed to create telemetry file {name}: {e}");
                None
            },
        }
    }
}

impl<S: Subscriber> Layer<S> for TelemetryLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        if event.metadata().target() != "telemetry" {
            return;
        }

        let ts = Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ");
        let mut visitor = JsonVisitor::new();
        event.record(&mut visitor);

        let mut line = format!("{{\"ts\":\"{ts}\"");
        line.push_str(&visitor.fields);
        line.push_str("}\n");

        if let Ok(mut w) = self.writer.lock() {
            let _ = w.write_all(line.as_bytes());
        }
    }
}

struct JsonVisitor {
    fields: String,
}

impl JsonVisitor {
    fn new() -> Self { Self { fields: String::new() } }
}

impl Visit for JsonVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        let escaped = value.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n");
        self.fields.push_str(&format!(",\"{}\":\"{}\"", field.name(), escaped));
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let s = format!("{value:?}");
        let escaped = s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n");
        self.fields.push_str(&format!(",\"{}\":\"{}\"", field.name(), escaped));
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.fields.push_str(&format!(",\"{}\":{}", field.name(), value));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.fields.push_str(&format!(",\"{}\":{}", field.name(), value));
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        self.fields.push_str(&format!(",\"{}\":{:.3}", field.name(), value));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.fields.push_str(&format!(",\"{}\":{}", field.name(), value));
    }
}
```

- [ ] **Step 2: Add module to `common/frontend/src/lib.rs`**

```rust
#[cfg(feature = "logging-verbose")]
mod telemetry_layer;
#[cfg(feature = "logging-verbose")]
pub use telemetry_layer::TelemetryLayer;
```

- [ ] **Step 3: Check it compiles with and without the feature**
```bash
cargo check -p veloren-common-frontend 2>&1 | grep "^error" | head -10
cargo check -p veloren-common-frontend --features logging-verbose 2>&1 | grep "^error" | head -10
```

- [ ] **Step 4: Commit**
```bash
git add common/frontend/src/telemetry_layer.rs common/frontend/src/lib.rs
git commit -m "feat(logging): add TelemetryLayer for JSON Lines telemetry sink"
```

---

### Task 5: `init_split_logs()` — wire everything together

**Files:**
- Modify: `common/frontend/src/lib.rs`

This function replaces `init_stdout()` at call sites. It creates all active sinks, registers the subscriber, runs startup cleanup, and returns guards + the `has_errors` flag + the lifecycle manager.

- [ ] **Step 1: Add constants and `LogGuards` struct to `common/frontend/src/lib.rs`**

Add near the top of the file (after imports):
```rust
use std::sync::{Arc, atomic::AtomicBool};
use bounded_writer::{BoundedMakeWriter, CompressionGuard, Rotation};
use lifecycle::{ErrorDetectorLayer, LogLifecycleManager};
use std::time::Duration;

// Line limits per file type
const CLIENT_INFO_MAX_LINES: u64  = 5_000;
const CLIENT_ERR_MAX_LINES: u64   = 1_000;
const SERVER_INFO_MAX_LINES: u64  = 10_000;
const SERVER_ERR_MAX_LINES: u64   = 1_000;
const TELEMETRY_MAX_LINES: u64    = 20_000;

// Retention windows
const CLIENT_INFO_RETENTION: Duration = Duration::from_secs(24 * 3600);
const CLIENT_ERR_RETENTION: Duration  = Duration::from_secs(7 * 24 * 3600);
const SERVER_RETENTION: Duration      = Duration::from_secs(30 * 24 * 3600);

/// Holds all log-related guards. Drop order matters: flush before compress thread exits.
pub struct LogGuards {
    pub has_errors: Arc<AtomicBool>,
    pub lifecycle: LogLifecycleManager,
    _worker_guards: Vec<tracing_appender::non_blocking::WorkerGuard>,
    _compress_guards: Vec<CompressionGuard>,
}
```

- [ ] **Step 2: Add `init_split_logs()` function**

Add after the existing `init_stdout()` function:

```rust
/// Initialise the 3-sink logging system:
///  - terminal (always)
///  - `{prefix}_err.log` WARN+ERROR (always)
///  - `{prefix}_info.log` DEBUG+INFO (logging-verbose feature only)
///  - `{prefix}_telemetry.jsonl` JSON Lines (logging-verbose feature only)
///
/// `prefix` is `"client"` (voxygen) or `"server"` (server-cli).
/// Call this INSTEAD of `init_stdout()` / `init()`.
pub fn init_split_logs(prefix: &str, logs_dir: &Path) -> LogGuards {
    use tracing_subscriber::{filter::LevelFilter, prelude::*, registry};
    use tracing_subscriber::fmt::layer as fmt_layer;

    let is_server = prefix.starts_with("server");

    // Startup lifecycle cleanup
    let lifecycle = LogLifecycleManager::new(logs_dir.to_owned());
    let (info_ret, err_ret) = if is_server {
        (SERVER_RETENTION, SERVER_RETENTION)
    } else {
        (CLIENT_INFO_RETENTION, CLIENT_ERR_RETENTION)
    };
    lifecycle.cleanup_on_startup(info_ret, err_ret);

    // ErrorDetectorLayer
    let (error_detector, has_errors) = ErrorDetectorLayer::new();

    // Terminal writer (always, mirrors existing behavior)
    let (non_blocking_term, term_guard) = tracing_appender::non_blocking(
        termcolor::StandardStream::stdout(termcolor::ColorChoice::Auto)
    );

    // err sink (always)
    let err_max = if is_server { SERVER_ERR_MAX_LINES } else { CLIENT_ERR_MAX_LINES };
    let (err_writer, err_compress) = BoundedMakeWriter::new(
        logs_dir,
        &format!("{prefix}_err"),
        Rotation::Daily,
        err_max,
    );

    // Build filter (same default directives as existing init())
    let filter = build_default_filter();

    // Compose registry
    let registry = registry()
        .with(fmt_layer().with_writer(non_blocking_term).with_filter(filter))
        .with(fmt_layer().with_writer(err_writer).with_filter(LevelFilter::WARN))
        .with(error_detector);

    let mut worker_guards = vec![term_guard];
    let mut compress_guards = vec![err_compress];

    // info + telemetry sinks (logging-verbose only)
    #[cfg(feature = "logging-verbose")]
    {
        let info_max = if is_server { SERVER_INFO_MAX_LINES } else { CLIENT_INFO_MAX_LINES };
        let (info_writer, info_compress) = BoundedMakeWriter::new(
            logs_dir,
            &format!("{prefix}_info"),
            Rotation::Hourly,
            info_max,
        );
        compress_guards.push(info_compress);

        let registry = registry
            .with(fmt_layer().with_writer(info_writer).with_filter(LevelFilter::DEBUG));

        if let Some(telemetry) = TelemetryLayer::new(logs_dir, prefix) {
            registry.with(telemetry).init();
        } else {
            registry.init();
        }
    }

    #[cfg(not(feature = "logging-verbose"))]
    registry.init();

    LogGuards {
        has_errors,
        lifecycle,
        _worker_guards: worker_guards,
        _compress_guards: compress_guards,
    }
}

fn build_default_filter() -> tracing_subscriber::EnvFilter {
    let mut filter = tracing_subscriber::EnvFilter::default()
        .add_directive(tracing_subscriber::filter::LevelFilter::INFO.into());
    let directives = [
        "dot_vox::parser=warn",
        "veloren_common::trade=info",
        "veloren_world::sim=info",
        "veloren_world::civ=info",
        "veloren_world::site::economy=info",
        "veloren_server::events::entity_manipulation=info",
        "hyper=info",
        "prometheus_hyper=info",
        "mio::poll=info",
        "assets_manager::anycache=info",
        "polling::epoll=info",
        "h2=info",
        "tokio_util=info",
        "rustls=info",
        "naga=info",
        "wgpu_core=info",
        "wgpu_core::device=warn",
        "veloren_network_protocol=info",
        "quinn_proto::connection=info",
        "refinery_core::traits::divergent=off",
        "veloren_server::persistence::character=info",
        "veloren_server::settings=info",
        "veloren_query_server=info",
        "symphonia_format_ogg::demuxer=off",
        "symphonia_core::probe=off",
        "wgpu_hal::dx12::device=off",
    ];
    for s in directives {
        filter = filter.add_directive(s.parse().unwrap());
    }
    match std::env::var(RUST_LOG_ENV) {
        Ok(env) => {
            for s in env.split(',') {
                match s.parse() {
                    Ok(d) => filter = filter.add_directive(d),
                    Err(err) => eprintln!("WARN ignoring log directive: `{s}`: {err}"),
                }
            }
        },
        Err(_) => {},
    }
    filter
}
```

The `#[cfg]` blocks change the type of `registry` with each `.with()`. Use `.boxed()` to erase the type so both cfg branches produce the same `Box<dyn Subscriber + Send + Sync>`. **Replace the `#[cfg]` block above with this**:

```rust
    // logging-verbose adds info + telemetry sinks.
    // .boxed() erases the type so both cfg arms produce the same Box<dyn Subscriber>.
    #[cfg(feature = "logging-verbose")]
    let registry = {
        let info_max = if is_server { SERVER_INFO_MAX_LINES } else { CLIENT_INFO_MAX_LINES };
        let (info_writer, info_compress) = BoundedMakeWriter::new(
            logs_dir,
            &format!("{prefix}_info"),
            Rotation::Hourly,
            info_max,
        );
        compress_guards.push(info_compress);
        let with_info = registry.with(fmt_layer().with_writer(info_writer).with_filter(LevelFilter::DEBUG));
        match TelemetryLayer::new(logs_dir, prefix) {
            Some(t) => with_info.with(t).boxed(),
            None    => with_info.boxed(),
        }
    };
    #[cfg(not(feature = "logging-verbose"))]
    let registry = registry.boxed();

    registry.init();

    LogGuards {
        has_errors,
        lifecycle,
        _worker_guards: worker_guards,
        _compress_guards: compress_guards,
    }
}
```

- [ ] **Step 3: Check compilation**
```bash
cargo check -p veloren-common-frontend 2>&1 | grep "^error" | head -20
cargo check -p veloren-common-frontend --features logging-verbose 2>&1 | grep "^error" | head -20
```

Fix any type errors. Common issue: `tracing_appender::non_blocking` takes a `Write` not a closure — use `termcolor::StandardStream::stdout(ColorChoice::Auto)` directly (not `||`).

- [ ] **Step 4: Commit**
```bash
git add common/frontend/src/lib.rs
git commit -m "feat(logging): add init_split_logs() wiring all log sinks"
```

---

### Task 6: Update `voxygen/src/main.rs` call site

**Files:**
- Modify: `voxygen/src/main.rs`

Replace the existing `init_stdout` call with `init_split_logs`. Also wire the clean-exit lifecycle hook.

- [ ] **Step 1: Replace logging init in `voxygen/src/main.rs`**

Current code at lines 77–88:
```rust
    // Determine where Voxygen's logs should go
    let logs_dir = std::env::var_os("VOXYGEN_LOGS")
        .map(PathBuf::from)
        .unwrap_or_else(|| userdata_dir.join("voxygen").join("logs"));

    // Init logging and hold the guards.
    let now = Utc::now();
    let log_filename = format!("{}_voxygen.log", now.format("%Y-%m-%d"));
    let _guards = common_frontend::init_stdout(Some((&logs_dir, &log_filename)));
```

Replace with:
```rust
    // Determine where Voxygen's logs should go
    let logs_dir = std::env::var_os("VOXYGEN_LOGS")
        .map(PathBuf::from)
        .unwrap_or_else(|| userdata_dir.join("voxygen").join("logs"));

    // Init split logging (err always, info+telemetry with logging-verbose feature).
    let log_guards = common_frontend::init_split_logs("client", &logs_dir);
```

- [ ] **Step 2: Remove the now-unused `log_filename` variable**

The `log_filename` was passed to `panic_handler::set_panic_hook(log_filename, logs_dir)` at line 128. Update that call to pass the err log path instead:

```rust
    panic_handler::set_panic_hook(
        format!("{}_client_err.log", chrono::Utc::now().format("%Y-%m-%d")),
        logs_dir.clone(),
    );
```

- [ ] **Step 3: Wire the clean-exit hook**

Near the end of `main()`, find where the game shuts down. Look for where `_guards` was dropped before. Add, just before the function returns:

```rust
    // Clean-exit: delete info/telemetry logs if session had no errors
    log_guards.lifecycle.cleanup_on_exit(&log_guards.has_errors);
```

- [ ] **Step 4: Check compilation**
```bash
source "$HOME/.cargo/env"
cargo check -p veloren-voxygen 2>&1 | grep "^error" | head -20
cargo check -p veloren-voxygen --features veloren-voxygen/logging-verbose 2>&1 | grep "^error" | head -20
```

- [ ] **Step 5: Commit**
```bash
git add voxygen/src/main.rs
git commit -m "feat(logging): switch voxygen to init_split_logs"
```

---

### Task 7: Update `server-cli/src/main.rs` call site

**Files:**
- Modify: `server-cli/src/main.rs`

Add file logging to the server. In basic mode, replace `init_stdout(None)`. TUI mode retains the existing TUI logger but adds file logging via a secondary subscriber (not possible with tracing — so TUI mode gets the file logging too by replacing `init()` with a combined approach).

- [ ] **Step 1: Add the logs_dir variable before the logging init**

Find the block around line 67:
```rust
    let shutdown_signal = Arc::new(AtomicBool::new(false));

    let (_guards, _guards2) = if basic {
        (Vec::new(), common_frontend::init_stdout(None))
    } else {
        (common_frontend::init(None, &|| LOG.clone()), Vec::new())
    };
```

Replace with:
```rust
    let shutdown_signal = Arc::new(AtomicBool::new(false));

    let server_logs_dir = common_base::userdata_dir().join("server").join("logs");

    // Always use split logging for file sinks.
    // TUI mode: the TUI log display is handled separately via LOG static after init.
    let _log_guards = common_frontend::init_split_logs("server", &server_logs_dir);
```

Note: this drops the TUI logger. To keep the TUI display functional, check if `tuilog.rs` uses the tracing subscriber or a separate mechanism. If it relies on the subscriber, you may need to add the TUI layer inside `init_split_logs` or accept that TUI display is lost (acceptable for now since file logging is the goal).

Actually, inspect how TuiLog works:
```bash
cat server-cli/src/tuilog.rs | head -50
```
If `TuiLog` implements `std::io::Write` (not a tracing Layer), it can be passed to `init()`. Since we can't call `init()` twice, the simplest fix is to accept that in this plan TUI mode also uses `init_split_logs` and the TUI visual display is unaffected (it reads logs via a separate channel, not via tracing). Verify this before committing.

- [ ] **Step 2: Check that server-cli compiles**
```bash
cargo check -p veloren-server-cli 2>&1 | grep "^error" | head -20
cargo check -p veloren-server-cli --features veloren-server-cli/logging-verbose 2>&1 | grep "^error" | head -20
```

Fix any import issues (add `use common_base;` if missing).

- [ ] **Step 3: Commit**
```bash
git add server-cli/src/main.rs
git commit -m "feat(logging): add file logging to server-cli via init_split_logs"
```

---

### Task 8: Integration test — verify log files are created

This task verifies the whole system works end-to-end.

- [ ] **Step 1: Build client with logging-verbose**
```bash
source "$HOME/.cargo/env"
cargo build --bin veloren-voxygen --features veloren-voxygen/logging-verbose 2>&1 | tail -5
```
Expected: `Finished dev profile`.

- [ ] **Step 2: Run for 10 seconds, then quit**
```bash
./target/debug/veloren-voxygen > /tmp/vox-split-test.log 2>&1 &
VPX_PID=$!
sleep 10
kill $VPX_PID
```

- [ ] **Step 3: Verify log files exist**
```bash
ls -la userdata/voxygen/logs/ | grep -E "client_err|client_info|telemetry"
```
Expected output (approximate):
```
-rw-r--r--  1 user  staff  ...  2026-06-05_14h_client_info.log
-rw-r--r--  1 user  staff  ...  2026-06-05_client_err.log
-rw-r--r--  1 user  staff  ...  2026-06-05_14h_client_telemetry.jsonl
```

- [ ] **Step 4: Verify err log has content (startup INFO → err is WARN-only, may be empty)**
```bash
wc -l userdata/voxygen/logs/*client_err* userdata/voxygen/logs/*client_info*
```

- [ ] **Step 5: Verify telemetry file is valid JSON Lines (even if empty)**
```bash
# Should produce no errors (empty file is valid)
python3 -c "
import json
with open(next(__import__('glob').iglob('userdata/voxygen/logs/*telemetry*'))) as f:
    for i, line in enumerate(f):
        json.loads(line)  # raises if invalid
print(f'OK: {i+1} events' if i >= 0 else 'OK: 0 events')
" 2>/dev/null || echo "File empty or not found"
```

- [ ] **Step 6: Build without logging-verbose and verify only err log is created**
```bash
cargo build --bin veloren-voxygen 2>&1 | tail -3
./target/debug/veloren-voxygen > /tmp/vox-noverbose.log 2>&1 &
sleep 5; kill %1
ls userdata/voxygen/logs/
```
Expected: only `*_client_err.log` (no info, no telemetry).

- [ ] **Step 7: Run clippy to catch any issues**
```bash
cargo clippy -p veloren-common-frontend -p veloren-voxygen -p veloren-server-cli -- -D warnings 2>&1 | grep "^error" | head -20
```

- [ ] **Step 8: Commit if any small fixes were needed; push**
```bash
git add -u
git commit -m "fix(logging): post-integration fixes" || echo "nothing to fix"
git push origin main
```

---

## Post-plan notes

- **Plan B** (logging coverage — adding `trace!`/`debug!`/`info!` calls throughout the codebase) depends on this plan being complete.
- **Plan C** (Report Bug button) also depends on this plan.
- The `telemetry!` convenience macro for emitting `trace!(target: "telemetry", ...)` events will be defined in Plan B as part of the first telemetry instrumentation task.
- Size-based rotation in `BoundedWriter::drop()` acquires the Mutex on every write — acceptable for log throughput, but if profiling shows lock contention, consider moving to a dedicated writer thread.
