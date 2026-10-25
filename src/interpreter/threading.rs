use super::*;
use crate::threads::{self, NativeThread, WaitOutcome};

#[derive(Clone, Copy, Debug)]
pub(super) enum ThreadOp {
    Initialize,
    Start,
    Join,
    State,
    IsBackground,
    SetBackground,
    ApartmentState,
    SetApartmentState,
    Sleep,
    Yield,
    PollGc,
    Ignore,
    False,
    TryEnter,
    TryEnterTimeout,
    ExitFast,
    IsEntered,
    EnterBlocking,
    TryEnterBlocking,
    ExitSlow,
    Wait,
    Pulse(bool),
    ReliableEnter,
    Enter,
    Exit,
}

pub(super) fn thread_internal(type_name: &str, name: &str, count: usize) -> Option<ThreadOp> {
    Some(match (type_name, name, count) {
        ("System.Threading.Monitor", "TryEnter_FastPath", 1) => ThreadOp::TryEnter,
        ("System.Threading.Monitor", "TryEnter_FastPath_WithTimeout", 2) => ThreadOp::TryEnterTimeout,
        ("System.Threading.Monitor", "Exit_FastPath", 1) => ThreadOp::ExitFast,
        ("System.Threading.Monitor", "IsEnteredNative", 1) => ThreadOp::IsEntered,
        ("System.Threading.Monitor", "ReliableEnter" | "ReliableEnterTimeout", 2 | 3) => ThreadOp::ReliableEnter,
        ("System.Threading.Monitor", "Enter", 1) => ThreadOp::Enter,
        ("System.Threading.Monitor", "Exit", 1) => ThreadOp::Exit,
        ("System.Threading.Thread", "PollGCInternal" | "FastPollGC", 0) => ThreadOp::PollGc,
        ("System.Threading.Thread", "CatchAtSafePoint", 0) => ThreadOp::False,
        ("System.Threading.Thread", "InternalFinalize", 1) => ThreadOp::Ignore,
        _ => return None,
    })
}

pub(super) fn thread_qcall(entry: &str, count: usize) -> Option<ThreadOp> {
    Some(match (entry, count) {
        ("ThreadNative_Initialize", 1) => ThreadOp::Initialize,
        ("ThreadNative_Start", 5) => ThreadOp::Start,
        ("ThreadNative_Join", 2) => ThreadOp::Join,
        ("ThreadNative_GetThreadState", 1) => ThreadOp::State,
        ("ThreadNative_GetIsBackground", 1) => ThreadOp::IsBackground,
        ("ThreadNative_SetIsBackground", 2) => ThreadOp::SetBackground,
        ("ThreadNative_GetApartmentState", 1) => ThreadOp::ApartmentState,
        ("ThreadNative_SetApartmentState", 2) => ThreadOp::SetApartmentState,
        ("ThreadNative_Sleep", 1) => ThreadOp::Sleep,
        ("ThreadNative_YieldThread", 0) => ThreadOp::Yield,
        ("ThreadNative_InformThreadNameChange", 3)
        | ("ThreadNative_SetPriority", 2)
        | ("ThreadNative_Interrupt", 1)
        | ("ThreadNative_DisableComObjectEagerCleanup", 1) => ThreadOp::Ignore,
        ("Monitor_Enter_Slowpath", 1) => ThreadOp::EnterBlocking,
        ("Monitor_TryEnter_Slowpath", 2) => ThreadOp::TryEnterBlocking,
        ("Monitor_Exit_Slowpath", 2) => ThreadOp::ExitSlow,
        ("Monitor_Wait", 2) => ThreadOp::Wait,
        ("Monitor_Pulse", 1) => ThreadOp::Pulse(false),
        ("Monitor_PulseAll", 1) => ThreadOp::Pulse(true),
        _ => return None,
    })
}

const STATE_BACKGROUND: i32 = 4;
const STATE_UNSTARTED: i32 = 8;
const STATE_STOPPED: i32 = 16;
const LEAVE_ERROR: i32 = 4;

fn native_thread(pointer: u64) -> Option<&'static NativeThread> {
    (pointer != 0).then(|| unsafe { &*(pointer as *const NativeThread) })
}

impl Interpreter<'_> {
    fn on_stack(value: V) -> u64 {
        let slot = unsafe { *(Self::struct_data(value) as *const u64) };
        if slot == 0 { 0 } else { unsafe { *(slot as *const u64) } }
    }

    fn thread_handle(value: V) -> u64 {
        unsafe { *(Self::struct_data(value) as *const u64) }
    }

    fn thread_field(&self, field: &str) -> Run<u64> {
        self.class_field_offset("System.Threading", "Thread", field).map(u64::from)
    }

    fn native_of(&self, object: u64) -> Run<Option<&'static NativeThread>> {
        if object == 0 {
            return Ok(None);
        }
        let offset = self.thread_field("_DONT_USE_InternalThread")?;
        Ok(native_thread(unsafe { *((object + offset) as *const u64) }))
    }

    fn attach_thread(&self, object: u64, managed_id: i32) -> Run<&'static NativeThread> {
        if let Some(existing) = self.native_of(object)? {
            return Ok(existing);
        }
        let slot = runtime::state().allocate_handle(object);
        let native = NativeThread::new(object, slot, managed_id);
        let pointer = native as *const NativeThread as u64;
        unsafe {
            *((object + self.thread_field("_DONT_USE_InternalThread")?) as *mut u64) = pointer;
            *((object + self.thread_field("_managedThreadId")?) as *mut i32) = managed_id;
        }
        Ok(native)
    }

    pub(super) fn current_thread(&self) -> Run<u64> {
        let roots = runtime::roots();
        if roots.thread_object != 0 {
            return Ok(roots.thread_object);
        }
        let handle = self.corelib_type("System.Threading", "Thread")?;
        let exact = Gen::Named(handle, Rc::from(Vec::new()));
        let object = runtime::allocate_managed(self.record_of(&exact)?, 0);
        if object == 0 {
            return self.throw("OutOfMemoryException");
        }
        roots.thread_object = object;
        let foreign = MAIN_ATTACHED.swap(true, std::sync::atomic::Ordering::SeqCst);
        let native = self.attach_thread(object, if foreign { threads::next_managed_id() } else { 1 })?;
        native.started.set(true);
        native.background.set(foreign);
        native.apartment.set(crate::pinvoke::apartment_state());
        Ok(object)
    }

    pub(super) fn current_managed_id(&self) -> Run<i32> {
        let object = self.current_thread()?;
        Ok(self.native_of(object)?.map_or(1, |native| native.managed_id))
    }

    pub(super) fn thread_call(&self, operation: ThreadOp, a: &[V]) -> Run<Option<V>> {
        Ok(Some(match operation {
            ThreadOp::Ignore => return Ok(None),
            ThreadOp::False => V::I4(0),
            ThreadOp::Initialize => {
                let object = self.non_null(V::O(Self::on_stack(a[0])))?;
                self.attach_thread(object, threads::next_managed_id())?;
                return Ok(None);
            }
            ThreadOp::Start => {
                let native = native_thread(Self::thread_handle(a[0]))
                    .ok_or_else(|| Fault::Unsupported("starting a thread the fallback did not create".into()))?;
                self.start_thread(native)?;
                return Ok(None);
            }
            ThreadOp::Join => {
                let object = self.non_null(V::O(Self::on_stack(a[0])))?;
                let joined = match self.native_of(object)? {
                    Some(native) if native.started.get() => native.join(a[1].int() as i32),
                    _ => true,
                };
                V::I4(i32::from(joined))
            }
            ThreadOp::State => {
                let state = match native_thread(Self::thread_handle(a[0])) {
                    Some(native) => {
                        let mut state = if !native.started.get() {
                            STATE_UNSTARTED
                        } else if native.is_finished() {
                            STATE_STOPPED
                        } else {
                            0
                        };
                        if native.background.get() {
                            state |= STATE_BACKGROUND;
                        }
                        state
                    }
                    None => 0,
                };
                V::I4(state)
            }
            ThreadOp::IsBackground => V::I4(i32::from(native_thread(Self::thread_handle(a[0])).is_some_and(|n| n.background.get()))),
            ThreadOp::SetBackground => {
                if let Some(native) = native_thread(Self::thread_handle(a[0])) {
                    native.background.set(a[1].int() != 0);
                }
                return Ok(None);
            }
            ThreadOp::ApartmentState => {
                let object = Self::on_stack(a[0]);
                match self.native_of(object)? {
                    Some(native) if object != runtime::roots().thread_object => V::I4(native.apartment.get()),
                    _ => V::I4(crate::pinvoke::apartment_state()),
                }
            }
            ThreadOp::SetApartmentState => {
                let object = Self::on_stack(a[0]);
                let requested = a[1].int() as i32;
                match self.native_of(object)? {
                    Some(native) if !native.started.get() => {
                        native.apartment.set(requested);
                        V::I4(requested)
                    }
                    _ if object == runtime::roots().thread_object => V::I4(crate::pinvoke::enter_apartment(requested)),
                    Some(native) => V::I4(native.apartment.get()),
                    None => V::I4(requested),
                }
            }
            ThreadOp::Sleep => {
                let milliseconds = a[0].int() as i32;
                if milliseconds == 0 {
                    threads::yield_now();
                } else {
                    let duration = if milliseconds < 0 {
                        std::time::Duration::from_secs(u64::from(u32::MAX))
                    } else {
                        std::time::Duration::from_millis(milliseconds as u64)
                    };
                    threads::blocking(|| std::thread::sleep(duration));
                }
                return Ok(None);
            }
            ThreadOp::Yield => {
                threads::yield_now();
                V::I4(1)
            }
            ThreadOp::PollGc => {
                threads::yield_now();
                return Ok(None);
            }
            ThreadOp::TryEnter => V::I4(i32::from(threads::try_enter(self.non_null(a[0])?))),
            ThreadOp::TryEnterTimeout => {
                let object = self.non_null(a[0])?;
                V::I4(if threads::try_enter(object) {
                    1
                } else if a[1].int() == 0 {
                    0
                } else {
                    2
                })
            }
            ThreadOp::ExitFast => V::I4(if threads::exit(self.non_null(a[0])?) { 0 } else { LEAVE_ERROR }),
            ThreadOp::IsEntered => V::I4(i32::from(threads::is_entered(self.non_null(a[0])?))),
            ThreadOp::EnterBlocking => {
                threads::enter(self.non_null(V::O(Self::on_stack(a[0])))?, -1);
                return Ok(None);
            }
            ThreadOp::TryEnterBlocking => V::I4(i32::from(threads::enter(self.non_null(V::O(Self::on_stack(a[0])))?, a[1].int() as i32))),
            ThreadOp::ExitSlow => {
                if a[1].int() as i32 == LEAVE_ERROR {
                    return self.throw("SynchronizationLockException");
                }
                return Ok(None);
            }
            ThreadOp::Wait => {
                let object = self.non_null(V::O(Self::on_stack(a[0])))?;
                match threads::wait(object, a[1].int() as i32) {
                    WaitOutcome::Signaled => V::I4(1),
                    WaitOutcome::TimedOut => V::I4(0),
                    WaitOutcome::NotOwner => return self.throw("SynchronizationLockException"),
                }
            }
            ThreadOp::Pulse(all) => {
                let object = self.non_null(V::O(Self::on_stack(a[0])))?;
                if !threads::pulse(object, all) {
                    return self.throw("SynchronizationLockException");
                }
                return Ok(None);
            }
            ThreadOp::ReliableEnter => {
                let object = self.non_null(a[0])?;
                let timeout = if a.len() == 3 { a[1].int() as i32 } else { -1 };
                let taken = threads::enter(object, timeout);
                unsafe { *(a[a.len() - 1].address() as *mut u8) = u8::from(taken) };
                return Ok(None);
            }
            ThreadOp::Enter => {
                threads::enter(self.non_null(a[0])?, -1);
                return Ok(None);
            }
            ThreadOp::Exit => {
                if !threads::exit(self.non_null(a[0])?) {
                    return self.throw("SynchronizationLockException");
                }
                return Ok(None);
            }
        }))
    }

    fn start_thread(&self, native: &'static NativeThread) -> Run<()> {
        if native.started.replace(true) {
            return self.throw("ThreadStateException");
        }
        let callback = self
            .find_method("System.Threading", "Thread", "StartCallback", 0)
            .ok_or_else(|| Fault::Unsupported("Thread has no StartCallback".into()))?;
        let interpreter = self as *const Interpreter<'_> as usize;
        let address = native as *const NativeThread as usize;
        let spawned = std::thread::Builder::new()
            .stack_size(threads::THREAD_STACK)
            .spawn(move || {
                let interpreter = unsafe { &*(interpreter as *const Interpreter<'static>) };
                let native = unsafe { &*(address as *const NativeThread) };
                threads::acquire();
                runtime::roots().thread_object = native.object;
                if native.apartment.get() != 2 {
                    crate::pinvoke::enter_apartment(native.apartment.get());
                }
                let outcome = interpreter.invoke(&callback, &[V::O(native.object)]);
                native.finish();
                runtime::state().free_handle(native.handle_slot);
                if let Err(fault) = outcome {
                    let code = crate::host::report(interpreter, fault);
                    std::process::exit(code);
                }
                runtime::retire_roots();
                threads::release();
            });
        if spawned.is_err() {
            native.started.set(false);
            return self.throw("OutOfMemoryException");
        }
        Ok(())
    }
}

static MAIN_ATTACHED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
