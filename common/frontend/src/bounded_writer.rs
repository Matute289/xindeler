use chrono::Utc;
use flate2::{Compression, write::GzEncoder};
use std::{
    fs::{self, File},
    io::{self, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    thread,
};
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
        if let Some(h) = self.0.take() {
            let _ = h.join();
        }
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
        // Find the highest existing seq for this bucket to avoid overwriting
        let seq = find_next_seq(base_dir, prefix, &bucket);
        let (path, writer) = open_log_file(base_dir, prefix, &bucket, seq).unwrap_or_else(|e| {
            eprintln!(
                "[log] cannot create log file in {}: {e}",
                base_dir.display()
            );
            // Fall back to /dev/null equivalent: create a temp file
            let path = std::env::temp_dir().join(format!("{bucket}_{prefix}_fallback.log"));
            let file = File::create(&path).expect("cannot create fallback log");
            (path, BufWriter::new(file))
        });
        let state = Arc::new(Mutex::new(WriterState {
            writer,
            path,
            line_count: 0,
            seq,
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
        Rotation::Daily => now.format("%Y-%m-%d").to_string(),
    }
}

fn open_log_file(
    dir: &Path,
    prefix: &str,
    bucket: &str,
    seq: u32,
) -> io::Result<(PathBuf, BufWriter<File>)> {
    fs::create_dir_all(dir)?;
    let name = if seq == 1 {
        format!("{bucket}_{prefix}.log")
    } else {
        format!("{bucket}_{prefix}.{seq}.log")
    };
    let path = dir.join(&name);
    let file = File::create(&path)?;
    Ok((path, BufWriter::new(file)))
}

/// Scan the directory for existing files matching the current bucket and return
/// `max_found_seq + 1` so we never overwrite a previous run's logs.
fn find_next_seq(dir: &Path, prefix: &str, bucket: &str) -> u32 {
    let Ok(entries) = fs::read_dir(dir) else {
        return 1;
    };
    let base = format!("{bucket}_{prefix}");
    let mut max_seq = 0u32;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(&base) && (name.ends_with(".log") || name.ends_with(".log.gz")) {
            max_seq = max_seq.max(1);
            // Try to parse seq number: "{base}.{seq}.log[.gz]"
            let stripped = name
                .strip_prefix(base.as_str())
                .and_then(|s| s.strip_suffix(".log").or_else(|| s.strip_suffix(".log.gz")));
            if let Some(mid) = stripped
                && let Some(seq_str) = mid.strip_prefix('.')
                && let Ok(n) = seq_str.parse::<u32>()
            {
                max_seq = max_seq.max(n);
            }
        }
    }
    if max_seq == 0 { 1 } else { max_seq + 1 }
}

fn compress_file(path: &Path) -> io::Result<()> {
    let gz_path = path.with_extension("log.gz");
    let src = fs::File::open(path)?;
    let mut reader = BufReader::new(src);
    let gz_file = fs::File::create(&gz_path)?;
    let mut enc = GzEncoder::new(BufWriter::new(gz_file), Compression::default());
    io::copy(&mut reader, &mut enc)?;
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

    fn flush(&mut self) -> io::Result<()> { self.state.writer.flush() }
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
            if self.compress_tx.try_send(old_path.clone()).is_err() {
                eprintln!(
                    "[log] compression queue full or disconnected, file will not be compressed: {}",
                    old_path.display()
                );
            }

            let (seq, bucket) = if needs_time_rotate {
                (1u32, new_bucket)
            } else {
                (self.state.seq + 1, self.state.bucket.clone())
            };
            match open_log_file(self.base_dir, self.prefix, &bucket, seq) {
                Ok((path, writer)) => {
                    self.state.writer = writer;
                    self.state.path = path;
                    self.state.line_count = 0;
                    self.state.seq = seq;
                    self.state.bucket = bucket;
                },
                Err(e) => {
                    // Cannot open new file — keep writing to the old one
                    eprintln!("[log] rotation failed, keeping old log file: {e}");
                },
            }
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
