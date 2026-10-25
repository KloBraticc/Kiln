use std::cell::Cell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};
use std::sync::{Condvar, LazyLock, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

struct Lock {
    held: bool,
    generation: u64,
}

static LOCK: Mutex<Lock> = Mutex::new(Lock { held: false, generation: 0 });
static FREED: Condvar = Condvar::new();
static WAITING: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    static HOLDING: Cell<bool> = const { Cell::new(false) };
}

fn guard<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

pub fn holding() -> bool {
    HOLDING.with(Cell::get)
}

pub fn acquire() {
    if holding() {
        return;
    }
    let mut lock = guard(&LOCK);
    WAITING.fetch_add(1, Ordering::SeqCst);
    while lock.held {
        lock = FREED.wait(lock).unwrap_or_else(PoisonError::into_inner);
    }
    WAITING.fetch_sub(1, Ordering::SeqCst);
    lock.held = true;
    lock.generation += 1;
    HOLDING.with(|h| h.set(true));
}

pub fn release() {
    if !holding() {
        return;
    }
    let mut lock = guard(&LOCK);
    lock.held = false;
    HOLDING.with(|h| h.set(false));
    drop(lock);
    FREED.notify_all();
}

pub fn blocking<R>(work: impl FnOnce() -> R) -> R {
    let held = holding();
    if held {
        release();
    }
    let result = work();
    if held {
        acquire();
    }
    result
}

pub fn yield_now() {
    if !holding() || WAITING.load(Ordering::SeqCst) == 0 {
        return;
    }
    let generation = guard(&LOCK).generation;
    release();
    let mut lock = guard(&LOCK);
    let deadline = Instant::now() + Duration::from_millis(5);
    while lock.generation == generation && WAITING.load(Ordering::SeqCst) > 0 {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        lock = FREED.wait_timeout(lock, deadline - now).unwrap_or_else(PoisonError::into_inner).0;
    }
    drop(lock);
    acquire();
}

pub fn others_waiting() -> bool {
    WAITING.load(Ordering::Relaxed) > 0
}

static NEXT_ID: AtomicI32 = AtomicI32::new(2);

pub fn next_managed_id() -> i32 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

pub fn os_thread_id() -> u32 {
    crate::pinvoke::current_thread_id()
}

#[derive(Default)]
struct Monitor {
    owner: u32,
    count: u32,
    tickets: Vec<(u64, bool)>,
}

struct Monitors {
    table: HashMap<u64, Monitor>,
    next_ticket: u64,
}

static MONITORS: LazyLock<Mutex<Monitors>> = LazyLock::new(|| Mutex::new(Monitors { table: HashMap::new(), next_ticket: 1 }));
static CHANGED: Condvar = Condvar::new();

fn monitors() -> MutexGuard<'static, Monitors> {
    guard(&MONITORS)
}

fn take(table: &mut Monitors, object: u64, me: u32) -> bool {
    let monitor = table.table.entry(object).or_default();
    if monitor.count == 0 || monitor.owner == me {
        monitor.owner = me;
        monitor.count += 1;
        true
    } else {
        false
    }
}

pub fn try_enter(object: u64) -> bool {
    take(&mut monitors(), object, os_thread_id())
}

pub fn enter(object: u64, milliseconds: i32) -> bool {
    if try_enter(object) {
        return true;
    }
    if milliseconds == 0 {
        return false;
    }
    let me = os_thread_id();
    let deadline = (milliseconds > 0).then(|| Instant::now() + Duration::from_millis(milliseconds as u64));
    blocking(|| {
        let mut table = monitors();
        loop {
            if take(&mut *table, object, me) {
                return true;
            }
            match deadline {
                None => table = CHANGED.wait(table).unwrap_or_else(PoisonError::into_inner),
                Some(deadline) => {
                    let now = Instant::now();
                    if now >= deadline {
                        return false;
                    }
                    table = CHANGED.wait_timeout(table, deadline - now).unwrap_or_else(PoisonError::into_inner).0;
                }
            }
        }
    })
}

pub fn exit(object: u64) -> bool {
    let me = os_thread_id();
    let mut guard = monitors();
    let table = &mut *guard;
    let Some(monitor) = table.table.get_mut(&object) else {
        return false;
    };
    if monitor.count == 0 || monitor.owner != me {
        return false;
    }
    monitor.count -= 1;
    if monitor.count == 0 {
        monitor.owner = 0;
        if monitor.tickets.is_empty() {
            table.table.remove(&object);
        }
        CHANGED.notify_all();
    }
    true
}

pub fn is_entered(object: u64) -> bool {
    let me = os_thread_id();
    monitors().table.get(&object).is_some_and(|m| m.count > 0 && m.owner == me)
}

pub enum WaitOutcome {
    Signaled,
    TimedOut,
    NotOwner,
}

pub fn wait(object: u64, milliseconds: i32) -> WaitOutcome {
    let me = os_thread_id();
    let (ticket, saved) = {
        let mut guard = monitors();
        let table = &mut *guard;
        let ticket = table.next_ticket;
        let Some(monitor) = table.table.get_mut(&object) else {
            return WaitOutcome::NotOwner;
        };
        if monitor.count == 0 || monitor.owner != me {
            return WaitOutcome::NotOwner;
        }
        let saved = monitor.count;
        monitor.count = 0;
        monitor.owner = 0;
        monitor.tickets.push((ticket, false));
        table.next_ticket += 1;
        CHANGED.notify_all();
        (ticket, saved)
    };
    let deadline = (milliseconds >= 0).then(|| Instant::now() + Duration::from_millis(milliseconds as u64));
    blocking(|| {
        let mut table = monitors();
        let mut signaled = false;
        loop {
            let monitor = table.table.entry(object).or_default();
            if let Some(position) = monitor.tickets.iter().position(|(t, _)| *t == ticket) {
                if monitor.tickets[position].1 {
                    monitor.tickets.remove(position);
                    signaled = true;
                    break;
                }
            } else {
                break;
            }
            match deadline {
                None => table = CHANGED.wait(table).unwrap_or_else(PoisonError::into_inner),
                Some(deadline) => {
                    let now = Instant::now();
                    if now >= deadline {
                        let monitor = table.table.entry(object).or_default();
                        monitor.tickets.retain(|(t, _)| *t != ticket);
                        break;
                    }
                    table = CHANGED.wait_timeout(table, deadline - now).unwrap_or_else(PoisonError::into_inner).0;
                }
            }
        }
        loop {
            let monitor = table.table.entry(object).or_default();
            if monitor.count == 0 {
                monitor.owner = me;
                monitor.count = saved;
                break;
            }
            table = CHANGED.wait(table).unwrap_or_else(PoisonError::into_inner);
        }
        if signaled { WaitOutcome::Signaled } else { WaitOutcome::TimedOut }
    })
}

pub fn pulse(object: u64, all: bool) -> bool {
    let me = os_thread_id();
    let mut guard = monitors();
    let Some(monitor) = guard.table.get_mut(&object) else {
        return false;
    };
    if monitor.count == 0 || monitor.owner != me {
        return false;
    }
    for ticket in monitor.tickets.iter_mut().filter(|(_, signaled)| !signaled) {
        ticket.1 = true;
        if !all {
            break;
        }
    }
    CHANGED.notify_all();
    true
}

pub struct NativeThread {
    pub object: u64,
    pub handle_slot: u64,
    pub managed_id: i32,
    pub background: Cell<bool>,
    pub apartment: Cell<i32>,
    pub started: Cell<bool>,
    finished: Mutex<bool>,
    done: Condvar,
}

unsafe impl Sync for NativeThread {}
unsafe impl Send for NativeThread {}

impl NativeThread {
    pub fn new(object: u64, handle_slot: u64, managed_id: i32) -> &'static NativeThread {
        Box::leak(Box::new(NativeThread {
            object,
            handle_slot,
            managed_id,
            background: Cell::new(false),
            apartment: Cell::new(2),
            started: Cell::new(false),
            finished: Mutex::new(false),
            done: Condvar::new(),
        }))
    }

    pub fn finish(&self) {
        *guard(&self.finished) = true;
        self.done.notify_all();
    }

    pub fn is_finished(&self) -> bool {
        *guard(&self.finished)
    }

    pub fn join(&self, milliseconds: i32) -> bool {
        blocking(|| {
            let mut finished = guard(&self.finished);
            let deadline = (milliseconds >= 0).then(|| Instant::now() + Duration::from_millis(milliseconds as u64));
            while !*finished {
                match deadline {
                    None => finished = self.done.wait(finished).unwrap_or_else(PoisonError::into_inner),
                    Some(deadline) => {
                        let now = Instant::now();
                        if now >= deadline {
                            return false;
                        }
                        finished = self.done.wait_timeout(finished, deadline - now).unwrap_or_else(PoisonError::into_inner).0;
                    }
                }
            }
            true
        })
    }
}

pub const THREAD_STACK: usize = 256 * 1024 * 1024;
