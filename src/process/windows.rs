//! Windows: a thread waiting on the parent process handle, and a Job Object.
//!
//! - Following on our own behalf: get the parent's pid with `NtQueryInformationProcess`, open
//!   a handle right after startup (pids are reused, so from then on wait on the handle), and
//!   place a thread that waits for the parent's exit with `WaitForSingleObject`
//! - Following on the upstream's behalf: put the upstream into a Job Object with
//!   `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. When the proxy dies the Job handle closes and the
//!   kernel kills the upstream. The same on a normal exit of the proxy
//!
//! Only the needed functions are declared directly from `kernel32` / `ntdll`
//! (ADR 0012 decision C).

use std::ffi::c_void;
use std::io;
use std::os::windows::io::AsRawHandle;
use std::process::{Child, Command};
use std::ptr;
use std::sync::OnceLock;
use std::thread;

type Handle = *mut c_void;
type Bool = i32;

const SYNCHRONIZE: u32 = 0x0010_0000;
const INFINITE: u32 = 0xFFFF_FFFF;
const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x2000;
/// `JOBOBJECTINFOCLASS::JobObjectExtendedLimitInformation`
const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: i32 = 9;
/// `PROCESSINFOCLASS::ProcessBasicInformation`
const PROCESS_BASIC_INFORMATION_CLASS: i32 = 0;

#[repr(C)]
struct IoCounters {
    read_operation_count: u64,
    write_operation_count: u64,
    other_operation_count: u64,
    read_transfer_count: u64,
    write_transfer_count: u64,
    other_transfer_count: u64,
}

#[repr(C)]
struct JobObjectBasicLimitInformation {
    per_process_user_time_limit: i64,
    per_job_user_time_limit: i64,
    limit_flags: u32,
    minimum_working_set_size: usize,
    maximum_working_set_size: usize,
    active_process_limit: u32,
    affinity: usize,
    priority_class: u32,
    scheduling_class: u32,
}

#[repr(C)]
struct JobObjectExtendedLimitInformation {
    basic_limit_information: JobObjectBasicLimitInformation,
    io_info: IoCounters,
    process_memory_limit: usize,
    job_memory_limit: usize,
    peak_process_memory_used: usize,
    peak_job_memory_used: usize,
}

#[repr(C)]
struct ProcessBasicInformation {
    exit_status: i32,
    peb_base_address: *mut c_void,
    affinity_mask: usize,
    base_priority: i32,
    unique_process_id: usize,
    inherited_from_unique_process_id: usize,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateJobObjectW(attributes: *const c_void, name: *const u16) -> Handle;
    fn SetInformationJobObject(
        job: Handle,
        class: i32,
        information: *const c_void,
        length: u32,
    ) -> Bool;
    fn AssignProcessToJobObject(job: Handle, process: Handle) -> Bool;
    fn OpenProcess(desired_access: u32, inherit_handle: Bool, process_id: u32) -> Handle;
    fn WaitForSingleObject(handle: Handle, milliseconds: u32) -> u32;
    fn GetCurrentProcess() -> Handle;
    fn CloseHandle(handle: Handle) -> Bool;
}

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQueryInformationProcess(
        process: Handle,
        class: i32,
        information: *mut c_void,
        length: u32,
        return_length: *mut u32,
    ) -> i32;
}

/// A handle passed between threads. A kernel object handle may be used from any thread.
struct SendHandle(Handle);
// SAFETY: as stated above.
unsafe impl Send for SendHandle {}
unsafe impl Sync for SendHandle {}

impl SendHandle {
    /// Taken out through a method so that a closure captures the whole struct
    /// (capturing only the field gives a raw pointer, which is not Send).
    fn raw(&self) -> Handle {
        self.0
    }
}

/// The Job the upstream is put into. Kept open for the whole lifetime of the process; closing
/// it is left to the process exit (normal or unexpected). That is the signal that kills the
/// upstream.
static JOB: OnceLock<Option<SendHandle>> = OnceLock::new();

pub fn prepare_upstream(_cmd: &mut Command) {}

pub fn follow_upstream(child: &Child) {
    let Some(job) = JOB.get_or_init(create_job) else {
        return;
    };
    // SAFETY: both are valid handles. Failure is known from the return value.
    let assigned = unsafe { AssignProcessToJobObject(job.raw(), child.as_raw_handle() as Handle) };
    if assigned == 0 {
        eprintln!(
            "lsp-det: cannot put the upstream into the job object: {}; \
             it will not follow an abrupt death of lsp-det",
            io::Error::last_os_error()
        );
    }
}

fn create_job() -> Option<SendHandle> {
    // SAFETY: created unnamed with default attributes. Failure is known from NULL.
    let job = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
    if job.is_null() {
        eprintln!(
            "lsp-det: CreateJobObject failed: {}; the upstream will not follow an abrupt death of lsp-det",
            io::Error::last_os_error()
        );
        return None;
    }
    // SAFETY: the struct is valid for any bit pattern.
    let mut info: JobObjectExtendedLimitInformation = unsafe { std::mem::zeroed() };
    info.basic_limit_information.limit_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    // SAFETY: information points to size_of bytes of valid memory.
    let set = unsafe {
        SetInformationJobObject(
            job,
            JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
            (&info as *const JobObjectExtendedLimitInformation).cast(),
            std::mem::size_of::<JobObjectExtendedLimitInformation>() as u32,
        )
    };
    if set == 0 {
        eprintln!(
            "lsp-det: SetInformationJobObject failed: {}; the upstream will not follow an abrupt death of lsp-det",
            io::Error::last_os_error()
        );
        // SAFETY: close the handle we created ourselves, while no one else holds it.
        unsafe {
            CloseHandle(job);
        }
        return None;
    }
    Some(SendHandle(job))
}

pub fn exit_with_parent() {
    let Some(parent) = parent_process_id() else {
        eprintln!("lsp-det: cannot find the parent process; will not follow its death");
        return;
    };
    // SAFETY: the arguments are a constant and a pid. Failure is known from NULL.
    let handle = unsafe { OpenProcess(SYNCHRONIZE, 0, parent) };
    if handle.is_null() {
        eprintln!(
            "lsp-det: cannot open parent process {parent}: {}; will not follow its death",
            io::Error::last_os_error()
        );
        return;
    }
    let handle = SendHandle(handle);
    thread::spawn(move || {
        // SAFETY: wait on the opened handle without closing it.
        unsafe {
            WaitForSingleObject(handle.raw(), INFINITE);
        }
        // The Job Object takes the upstream down with us.
        eprintln!("lsp-det: parent process exited; exiting");
        std::process::exit(1);
    });
}

fn parent_process_id() -> Option<u32> {
    // SAFETY: the struct is valid for any bit pattern.
    let mut info: ProcessBasicInformation = unsafe { std::mem::zeroed() };
    let mut returned = 0u32;
    // SAFETY: information points to size_of bytes of valid memory.
    let status = unsafe {
        NtQueryInformationProcess(
            GetCurrentProcess(),
            PROCESS_BASIC_INFORMATION_CLASS,
            (&mut info as *mut ProcessBasicInformation).cast(),
            std::mem::size_of::<ProcessBasicInformation>() as u32,
            &mut returned,
        )
    };
    // A negative NTSTATUS is a failure.
    (status >= 0).then_some(info.inherited_from_unique_process_id as u32)
}
