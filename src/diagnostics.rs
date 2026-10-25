use std::collections::HashSet;
use std::fs::File;
use std::io::Write;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::Instant;

use crate::codegen::MethodKey;
use crate::metadata::TableId;
use crate::objects::ObjectModel;

struct Log {
    file: Mutex<File>,
    started: Instant,
    seen: Mutex<HashSet<String>>,
}

static LOG: OnceLock<Option<Log>> = OnceLock::new();
static MODEL: AtomicPtr<ObjectModel<'static>> = AtomicPtr::new(std::ptr::null_mut());

fn log_file() -> Option<&'static Log> {
    LOG.get_or_init(|| {
        let path = std::env::var_os("KILN_LOG")?;
        let file = File::options().create(true).append(true).open(path).ok()?;
        Some(Log { file: Mutex::new(file), started: Instant::now(), seen: Mutex::new(HashSet::new()) })
    })
    .as_ref()
}

pub fn enabled() -> bool {
    log_file().is_some()
}

pub fn attach(model: &ObjectModel<'_>) {
    MODEL.store((model as *const ObjectModel<'_>).cast_mut().cast(), Ordering::Release);
}

pub fn method_name(module: usize, row: u32) -> String {
    let model = MODEL.load(Ordering::Acquire);
    if model.is_null() {
        return format!("method {module}:{row}");
    }
    let model = unsafe { &*model };
    let key = MethodKey { module, row };
    let owner = model
        .method_owner(&key)
        .ok()
        .and_then(|owner| model.set.full_type_name(owner).ok())
        .unwrap_or_default();
    let name = model
        .set
        .modules
        .get(module)
        .and_then(|m| m.metadata.string_cell(TableId::MethodDef, row, 3).ok())
        .unwrap_or("?");
    format!("{owner}::{name}")
}

pub fn managed_stack(limit: usize) -> String {
    let calls = &crate::runtime::roots().calls;
    let mut text = String::new();
    for &(module, row) in calls.iter().rev().take(limit) {
        text.push_str("\n    at ");
        text.push_str(&method_name(module, row));
    }
    if calls.len() > limit {
        text.push_str(&format!("\n    ... {} more frames", calls.len() - limit));
    }
    text
}

pub fn write(kind: &str, message: &str) {
    let Some(log) = log_file() else {
        return;
    };
    let line = format!(
        "[{:>10.3}s] [thread {}] {kind}: {message}{}\n",
        log.started.elapsed().as_secs_f64(),
        crate::pinvoke::current_thread_id(),
        managed_stack(24)
    );
    let mut file = log.file.lock().unwrap_or_else(PoisonError::into_inner);
    let _ = file.write_all(line.as_bytes());
    let _ = file.flush();
}

pub fn write_once(kind: &str, message: &str) {
    let Some(log) = log_file() else {
        return;
    };
    let fresh = log.seen.lock().unwrap_or_else(PoisonError::into_inner).insert(format!("{kind}:{message}"));
    if fresh {
        write(kind, message);
    }
}
