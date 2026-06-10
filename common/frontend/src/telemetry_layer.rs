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
