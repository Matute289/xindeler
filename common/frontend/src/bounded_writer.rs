use chrono::Utc;
use flate2::{Compression, write::GzEncoder};
use std::{
    fs::{self, File},
    io::{self, BufWriter, Write},
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
