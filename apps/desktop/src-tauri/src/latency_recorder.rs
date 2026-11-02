use crate::{latency_measurement::B4bConfigurationV1, protocol::MotionSampleV1};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs::OpenOptions,
    io::{BufWriter, Write},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc,
    },
    thread,
};

pub const LATENCY_RECORDER_CAPACITY: usize = 512;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "recordType", rename_all = "camelCase", deny_unknown_fields)]
pub enum LatencyRecord {
    Metadata {
        latency_dataset_version: u8,
        measurement_protocol_version: u8,
        requested_duration_us: u64,
        b4b_configuration: B4bConfigurationV1,
    },
    Hello {
        measurement_protocol_version: u8,
        measurement_id: String,
        endpoint: String,
        desktop_ingress_us: u64,
    },
    ClockSyncRequestSent {
        measurement_protocol_version: u8,
        measurement_id: String,
        nonce: String,
        endpoint: String,
        t1_desktop_us: u64,
    },
    ClockSyncResponseReceived {
        measurement_protocol_version: u8,
        measurement_id: String,
        nonce: String,
        endpoint: String,
        t1_desktop_us: u64,
        t2_android_us: u64,
        t3_android_us: u64,
        t4_desktop_us: u64,
    },
    SampleAccepted {
        measurement_protocol_version: u8,
        measurement_id: String,
        session_id: String,
        sequence: u32,
        sample: MotionSampleV1,
        android_sensor_event_us: u64,
        android_send_preparation_us: u64,
        desktop_ingress_us: u64,
        desktop_accepted_us: u64,
    },
    SampleProcessed {
        measurement_protocol_version: u8,
        measurement_id: String,
        session_id: String,
        sequence: u32,
        desktop_processed_us: u64,
        processing_completed: bool,
        b4b_state: String,
    },
    Rejection {
        measurement_protocol_version: u8,
        measurement_id: Option<String>,
        desktop_us: u64,
        category: String,
        reason: String,
    },
    Summary {
        measurement_protocol_version: u8,
        measurement_id: String,
        completed: bool,
        declared_counters: Value,
    },
}

enum Command {
    Record(LatencyRecord),
    Finish(LatencyRecord),
}

#[derive(Default)]
struct Shared {
    dropped: AtomicU64,
    critical_dropped: AtomicU64,
    failed: AtomicBool,
}

#[derive(Clone)]
pub struct LatencyRecorderIngress {
    tx: mpsc::SyncSender<Command>,
    shared: Arc<Shared>,
}
impl LatencyRecorderIngress {
    pub fn try_publish(&self, record: LatencyRecord, critical: bool) -> bool {
        match self.tx.try_send(Command::Record(record)) {
            Ok(()) => true,
            Err(_) => {
                self.shared.dropped.fetch_add(1, Ordering::Relaxed);
                if critical {
                    self.shared.critical_dropped.fetch_add(1, Ordering::Relaxed);
                }
                false
            }
        }
    }
    pub fn dropped(&self) -> u64 {
        self.shared.dropped.load(Ordering::Relaxed)
    }
    pub fn critical_dropped(&self) -> u64 {
        self.shared.critical_dropped.load(Ordering::Relaxed)
    }
}

pub struct LatencyRecorder {
    ingress: LatencyRecorderIngress,
    join: thread::JoinHandle<Result<(), String>>,
}
impl LatencyRecorder {
    pub fn start(path: PathBuf, capacity: usize) -> Result<Self, String> {
        if path.exists() {
            return Err(format!("dataset output already exists: {}", path.display()));
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?
        }
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path)
            .map_err(|e| e.to_string())?;
        Ok(Self::from_writer(BufWriter::new(file), capacity))
    }

    fn from_writer<W: Write + Send + 'static>(mut writer: W, capacity: usize) -> Self {
        let (tx, rx) = mpsc::sync_channel(capacity);
        let shared = Arc::new(Shared::default());
        let worker_shared = shared.clone();
        let join = thread::spawn(move || {
            while let Ok(command) = rx.recv() {
                let (record, finish) = match command {
                    Command::Record(v) => (v, false),
                    Command::Finish(v) => (v, true),
                };
                if let Err(e) = serde_json::to_writer(&mut writer, &record)
                    .and_then(|_| writer.write_all(b"\n").map_err(serde_json::Error::io))
                {
                    worker_shared.failed.store(true, Ordering::Relaxed);
                    return Err(e.to_string());
                }
                if finish {
                    writer.flush().map_err(|e| e.to_string())?;
                    return Ok(());
                }
            }
            Err("recorder channel closed without summary".into())
        });
        Self {
            ingress: LatencyRecorderIngress { tx, shared },
            join,
        }
    }
    pub fn ingress(&self) -> LatencyRecorderIngress {
        self.ingress.clone()
    }
    pub fn shutdown(self, summary: LatencyRecord) -> Result<(), String> {
        self.ingress
            .tx
            .send(Command::Finish(summary))
            .map_err(|_| "recorder worker unavailable".to_owned())?;
        drop(self.ingress);
        self.join
            .join()
            .map_err(|_| "recorder worker panicked".to_owned())??;
        Ok(())
    }

    pub fn abort(self) -> Result<(), String> {
        drop(self.ingress);
        match self
            .join
            .join()
            .map_err(|_| "recorder worker panicked".to_owned())?
        {
            Err(error) if error == "recorder channel closed without summary" => Ok(()),
            result => result,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    struct FailingWriter;
    impl Write for FailingWriter {
        fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("injected write failure"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    struct BlockingWriter {
        entered: Option<mpsc::Sender<()>>,
        release: Option<mpsc::Receiver<()>>,
    }
    impl Write for BlockingWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if let Some(entered) = self.entered.take() {
                entered.send(()).unwrap();
                self.release.take().unwrap().recv().unwrap();
            }
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn bounded_queue_counts_drop() {
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let recorder = LatencyRecorder::from_writer(
            BlockingWriter {
                entered: Some(entered_tx),
                release: Some(release_rx),
            },
            1,
        );
        let ingress = recorder.ingress();
        let record = LatencyRecord::Rejection {
            measurement_protocol_version: 1,
            measurement_id: None,
            desktop_us: 0,
            category: "test".into(),
            reason: "test".into(),
        };
        assert!(ingress.try_publish(record.clone(), false));
        entered_rx.recv().unwrap();
        assert!(ingress.try_publish(record.clone(), false));
        assert!(!ingress.try_publish(record, true));
        assert_eq!(ingress.dropped(), 1);
        assert_eq!(ingress.critical_dropped(), 1);
        release_tx.send(()).unwrap();
        recorder
            .shutdown(LatencyRecord::Summary {
                measurement_protocol_version: 1,
                measurement_id: "test".into(),
                completed: false,
                declared_counters: serde_json::json!({}),
            })
            .unwrap();
    }

    #[test]
    fn worker_write_failure_is_propagated_by_shutdown() {
        let recorder = LatencyRecorder::from_writer(FailingWriter, 1);
        let error = recorder
            .shutdown(LatencyRecord::Summary {
                measurement_protocol_version: 1,
                measurement_id: "test".into(),
                completed: false,
                declared_counters: serde_json::json!({}),
            })
            .unwrap_err();
        assert!(error.contains("injected write failure"));
    }
}
