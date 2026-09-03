//! Windows: 親プロセスのハンドルを待つスレッドと、Job Object。
//!
//! - 自身の追従: 親の pid を `NtQueryInformationProcess` で取り、起動直後に
//!   ハンドルを開いて (pid は再利用されるので、以後はハンドルで待つ)、
//!   `WaitForSingleObject` で親の終了を待つスレッドを置く
//! - 上流の追従: `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` を付けた Job Object に
//!   上流を入れる。プロキシが死ぬと Job のハンドルが閉じ、カーネルが上流を
//!   殺す。プロキシの正常終了でも同じ
//!
//! 必要な関数だけを `kernel32` / `ntdll` から直接宣言する (ADR 0012 決定 C)。

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

/// スレッド間で持ち回るハンドル。カーネルオブジェクトのハンドルはどの
/// スレッドから使ってもよい。
struct SendHandle(Handle);
// SAFETY: 上記のとおり。
unsafe impl Send for SendHandle {}
unsafe impl Sync for SendHandle {}

impl SendHandle {
    /// クロージャが構造体ごと捕捉するように、メソッド経由で取り出す
    /// (フィールドだけを捕捉すると生ポインタになり Send でなくなる)。
    fn raw(&self) -> Handle {
        self.0
    }
}

/// 上流を入れる Job。プロセスの寿命の間ずっと開いたままにし、閉じるのは
/// プロセスの終了 (正常でも不意でも) に任せる。それが上流を殺す合図になる。
static JOB: OnceLock<Option<SendHandle>> = OnceLock::new();

pub fn prepare_upstream(_cmd: &mut Command) {}

pub fn follow_upstream(child: &Child) {
    let Some(job) = JOB.get_or_init(create_job) else {
        return;
    };
    // SAFETY: どちらも有効なハンドル。失敗は戻り値で分かる。
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
    // SAFETY: 名前なし・既定の属性で作る。失敗は NULL で分かる。
    let job = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
    if job.is_null() {
        eprintln!(
            "lsp-det: CreateJobObject failed: {}; the upstream will not follow an abrupt death of lsp-det",
            io::Error::last_os_error()
        );
        return None;
    }
    // SAFETY: 構造体はどのビットパターンでも有効。
    let mut info: JobObjectExtendedLimitInformation = unsafe { std::mem::zeroed() };
    info.basic_limit_information.limit_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    // SAFETY: information は size_of ぶんの有効なメモリを指す。
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
        // SAFETY: 自分で作ったハンドルを、他に持ち手がないうちに閉じる。
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
    // SAFETY: 引数は定数と pid。失敗は NULL で分かる。
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
        // SAFETY: 開いたハンドルを、閉じずに待つ。
        unsafe {
            WaitForSingleObject(handle.raw(), INFINITE);
        }
        // Job Object が上流を道連れにする。
        eprintln!("lsp-det: parent process exited; exiting");
        std::process::exit(1);
    });
}

fn parent_process_id() -> Option<u32> {
    // SAFETY: 構造体はどのビットパターンでも有効。
    let mut info: ProcessBasicInformation = unsafe { std::mem::zeroed() };
    let mut returned = 0u32;
    // SAFETY: information は size_of ぶんの有効なメモリを指す。
    let status = unsafe {
        NtQueryInformationProcess(
            GetCurrentProcess(),
            PROCESS_BASIC_INFORMATION_CLASS,
            (&mut info as *mut ProcessBasicInformation).cast(),
            std::mem::size_of::<ProcessBasicInformation>() as u32,
            &mut returned,
        )
    };
    // NTSTATUS は負なら失敗。
    (status >= 0).then_some(info.inherited_from_unique_process_id as u32)
}
