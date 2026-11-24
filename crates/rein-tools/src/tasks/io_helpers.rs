// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Task ID, timestamp formatting, ring-tail, and condvar helpers for TaskRegistry

use std::io::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rein_policy::local_time_parts;

use super::registry::{TASK_OUTPUT_CAP_BYTES, TaskEntry, TaskRuntime, TaskSummary};

static TASK_SEQ: AtomicU64 = AtomicU64::new(1);

pub(super) fn next_task_id() -> String {
    let seq = TASK_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("task-{seq}")
}

pub(super) fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

pub(super) fn format_local_timestamps(unix_secs: u64) -> (String, String) {
    let ts = i64::try_from(unix_secs).unwrap_or(0);
    let p = local_time_parts(ts);
    let hms = format!("{:02}:{:02}:{:02}", p.hour, p.minute, p.second);
    let full = format!("{:04}-{:02}-{:02} {hms}", p.year, p.month, p.day);
    (hms, full)
}

pub(super) fn format_duration_short(secs: u64) -> String {
    if secs.is_multiple_of(3_600) && secs >= 3_600 {
        format!("{}h", secs / 3_600)
    } else if secs.is_multiple_of(60) && secs >= 60 {
        format!("{}m", secs / 60)
    } else {
        format!("{secs}s")
    }
}

pub(super) fn refresh_summary_snapshot(entry: &TaskEntry) -> TaskSummary {
    let mut summary = entry.summary.clone();
    if let TaskRuntime::Shell { ring, .. } = &entry.runtime {
        summary.output_tail = read_ring_tail(ring, 40);
    }
    summary
}

pub(super) fn drain_split<R: std::io::Read>(
    pipe: Option<R>,
    header: &str,
    log: &rein_policy::SharedLogWriter,
    stream_buf: &Arc<Mutex<Vec<u8>>>,
    combined_ring: &Arc<Mutex<Vec<u8>>>,
) -> std::io::Result<()> {
    let Some(mut reader) = pipe else {
        return Ok(());
    };
    let mut buf = [0u8; 4096];
    let mut wrote_header = false;
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        let chunk = &buf[..n];
        if let Ok(mut guard) = log.lock() {
            if !wrote_header {
                guard.write_all(header.as_bytes())?;
                wrote_header = true;
            }
            guard.write_all(chunk)?;
            guard.flush()?;
        }
        append_capped(stream_buf, chunk, TASK_OUTPUT_CAP_BYTES);
        append_capped(combined_ring, chunk, TASK_OUTPUT_CAP_BYTES);
    }
    Ok(())
}

pub(super) fn append_capped(target: &Arc<Mutex<Vec<u8>>>, chunk: &[u8], cap: usize) {
    if let Ok(mut guard) = target.lock() {
        guard.extend_from_slice(chunk);
        if guard.len() > cap {
            let excess = guard.len() - cap;
            guard.drain(..excess);
        }
    }
}

pub(super) fn read_stream_buffer(buf: &Arc<Mutex<Vec<u8>>>) -> (String, bool) {
    let Ok(guard) = buf.lock() else {
        return (String::new(), false);
    };
    let truncated = guard.len() >= rein_policy::OUTPUT_CAP_BYTES;
    let slice = if guard.len() > rein_policy::OUTPUT_CAP_BYTES {
        &guard[guard.len() - rein_policy::OUTPUT_CAP_BYTES..]
    } else {
        &guard[..]
    };
    (String::from_utf8_lossy(slice).into_owned(), truncated)
}

pub(super) fn read_ring_tail(ring: &Arc<Mutex<Vec<u8>>>, max_lines: usize) -> String {
    let Ok(guard) = ring.lock() else {
        return String::new();
    };
    let text = String::from_utf8_lossy(&guard);
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(max_lines);
    lines[start..].join("\n")
}

pub(super) fn signal_condvar(pair: &(Mutex<bool>, Condvar)) {
    if let Ok(mut flag) = pair.0.lock() {
        *flag = true;
        pair.1.notify_all();
    }
}

pub(super) fn wait_condvar_timeout(pair: &(Mutex<bool>, Condvar), timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    let Ok(mut guard) = pair.0.lock() else {
        return false;
    };
    while !*guard {
        let now = Instant::now();
        if now >= deadline {
            return false;
        }
        let Ok((next, _)) = pair.1.wait_timeout(guard, deadline - now) else {
            return false;
        };
        guard = next;
    }
    true
}
