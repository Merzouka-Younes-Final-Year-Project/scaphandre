#![no_std]
#![no_main]

use aya_ebpf::{
    helpers::{bpf_get_current_pid_tgid, bpf_ktime_get_ns, bpf_probe_read_kernel, generated::bpf_get_smp_processor_id}, macros::{map, perf_event, raw_tracepoint, tracepoint},
    maps::{Array, PerCpuHashMap, RingBuf},
    programs::{PerfEventContext, RawTracePointContext, TracePointContext}
};
use scaphandre_common::{CpuEventType, CpuStateEvent};

mod vmlinux;
use vmlinux::task_struct;

const MAX_KEYS: u32 = 32768;
const MAX_CPU: u32 = 256;
const MAX_SOCKET: u16 = 64;

/// Layout from /sys/kernel/debug/tracing/events/sched/sched_process_exit/format
#[repr(C)]
struct SchedProcessExitArgs {
    common_type:     u16,
    common_flags:    u8,
    common_preempt:  u8,
    common_pid:      i32,
    comm:            [u8; 16],
    pid:             i32,
    prio:            i32,
    group_dead:      i32,
}

#[map]
static PID_TIMES: PerCpuHashMap<u32, u64> = PerCpuHashMap::with_max_entries(MAX_KEYS, 0);

#[map]
static PID_LAST: PerCpuHashMap<u32, u64> = PerCpuHashMap::with_max_entries(MAX_KEYS, 0);

#[map]
static CPU_TIME: Array<u64> = Array::with_max_entries(MAX_CPU, 0);

#[map]
static CPU_SNAPSHOT: Array<u64> = Array::with_max_entries(MAX_CPU, 0);

/// Ring buffer for CPU state events consumed by userspace. 256 KB.
#[map]
static CPU_STATE_EVENTS: RingBuf = RingBuf::with_byte_size(256 * 1024, 0);

#[map]
static CPU_TO_SOCKET: Array<u16> = Array::with_max_entries(MAX_CPU, 0);

#[raw_tracepoint(tracepoint = "sched_switch")]
pub fn context_switch_tracker(ctx: RawTracePointContext) -> u32 {
    match try_sched_switch(ctx) {
        Ok(ret) => ret,
        Err(ret) => ret,
    }
}

fn try_sched_switch(ctx: RawTracePointContext) -> Result<u32, u32> {
    let now = unsafe { bpf_ktime_get_ns() };
    let cpu = unsafe { bpf_get_smp_processor_id() };

    let prev = ctx.arg::<*const task_struct>(1);
    let next = ctx.arg::<*const task_struct>(2);

    let prev_tgid: u32 = unsafe { bpf_probe_read_kernel(&(*prev).tgid).map_err(|e| e as u32)? as u32 };
    let next_tgid: u32 = unsafe { bpf_probe_read_kernel(&(*next).tgid).map_err(|e| e as u32)? as u32 };
    let prev_tid: u32  = unsafe { bpf_probe_read_kernel(&(*prev).pid).map_err(|e| e as u32)? as u32 };
    let next_tid: u32  = unsafe { bpf_probe_read_kernel(&(*next).pid).map_err(|e| e as u32)? as u32 };



    // Accumulate time for the task that just got switched OFF the CPU
    // Skip the idle task (PID 0) so CPU_TIME only reflects non-idle runtime.
    if prev_tgid != 0 && prev_tid != 0 {
        if let Some(last_timestamp) = PID_LAST.get_ptr(prev_tgid) {
            let delta = now - unsafe { *last_timestamp };
            if let Some(p_time) = PID_TIMES.get_ptr_mut(prev_tgid) {
                unsafe { *p_time += delta };
            } else {
                let _ = PID_TIMES.insert(prev_tgid, delta, 0);
            };
            let _ = PID_LAST.remove(prev_tgid);
            if let Some(c_time) = CPU_TIME.get_ptr_mut(cpu) {
                unsafe { *c_time += delta };
            }
        }
    }

    // Record when the incoming task got switched ON
    let _ = PID_LAST.insert(next_tgid, now, 0);

    Ok(0)
}

#[perf_event]
pub fn sample_tick(_ctx: PerfEventContext) -> u32 {
    let now = unsafe { bpf_ktime_get_ns() };
    let pid = (bpf_get_current_pid_tgid() >> 32) as u32;
    let tid = (bpf_get_current_pid_tgid() & 0xFFFFFFFF) as u32;
    let cpu = unsafe { bpf_get_smp_processor_id() };

    if pid == 0 || tid == 0 {
        return 0;
    }

    if let Some(p) = PID_LAST.get_ptr_mut(pid) {
        let delta = now - unsafe { *p };
        if let Some(p_time) = PID_TIMES.get_ptr_mut(pid) {
            unsafe { *p_time += delta };
        } else {
            let _ = PID_TIMES.insert(pid, delta, 0);
        }
        if let Some(c_time) = CPU_TIME.get_ptr_mut(cpu) {
            unsafe { *c_time += delta };
        }
        unsafe { *p = now };
    } else {
        let _ = PID_LAST.insert(pid, now, 0);
    }

    0
}

/// Fires every 10 ms.
#[perf_event]
pub fn cpu_state_tick(_ctx: PerfEventContext) -> u32 {
    let mut socket_active_cpus: [u32; MAX_SOCKET as usize] = [u32::MAX; MAX_SOCKET as usize];
    for cpu in 0..MAX_CPU {
        if let Some(old) = CPU_SNAPSHOT.get(cpu) {
            if let Some(new) = CPU_TIME.get(cpu) {
                if let Some(socket) = CPU_TO_SOCKET.get(cpu) {
                    // This is so we can know if we have reached the end of the current system's
                    // CPUs userspace code starts socket ids from 1
                    if (*socket) == 0 {
                        break;
                    }
                    if let Some(active_cpus) = socket_active_cpus.get_mut((*socket as usize).saturating_sub(1)) {
                        if *active_cpus == u32::MAX {
                            *active_cpus = 0;
                        }
                        if new - old != 0 {
                            *active_cpus += 1;
                        }
                    }
                }
            }
        }
    }

    for socket in 0..MAX_SOCKET {
        let active_cpus = socket_active_cpus[socket as usize];
        if active_cpus == u32::MAX {
            break;
        }
        if active_cpus == 0 {
            if let Some(mut entry) = CPU_STATE_EVENTS.reserve::<CpuStateEvent>(0) {
                unsafe {
                    (*entry.as_mut_ptr()) = CpuStateEvent { 
                        socket_id: socket,
                        event_type: CpuEventType::IdleEvent,
                    };
                }
                entry.submit(0);
            }
        } else {
            if active_cpus == 1 {
                if let Some(mut entry) = CPU_STATE_EVENTS.reserve::<CpuStateEvent>(0) {
                    unsafe {
                        (*entry.as_mut_ptr()) = CpuStateEvent { 
                            socket_id: socket,
                            event_type: CpuEventType::ActivationEvent,
                        };
                    }
                    entry.submit(0);
                }
            }
        }
    }

    for cpu in 0..MAX_CPU {
        if let Some(c_time) = CPU_TIME.get(cpu) {
            let _ = CPU_SNAPSHOT.set(cpu, c_time, 0);
        }
    }

    0
}

#[tracepoint]
pub fn process_exit_cleanup(ctx: TracePointContext) -> u32 {
    match try_process_exit(ctx) {
        Ok(ret) => ret,
        Err(ret) => ret,
    }
}

fn try_process_exit(ctx: TracePointContext) -> Result<u32, u32> {
    let now = unsafe { bpf_ktime_get_ns() };
    let cpu = unsafe { bpf_get_smp_processor_id() };
    let tgid = (bpf_get_current_pid_tgid() >> 32) as u32;

    let args = unsafe {
        ctx.read_at::<SchedProcessExitArgs>(0).map_err(|_| 1u32)?
    };
    let tid = args.pid as u32;

    if tgid != 0 && tid != 0 {
        if let Some(last) = PID_LAST.get_ptr(tgid) {
            let delta = now - unsafe { *last };
            if let Some(p_time) = PID_TIMES.get_ptr_mut(tgid) {
                unsafe { *p_time += delta };
            } else {
                let _ = PID_TIMES.insert(tgid, delta, 0);
            };
            if let Some(c_time) = CPU_TIME.get_ptr_mut(cpu) {
                unsafe { *c_time += delta };
            }
            let _ = PID_LAST.remove(tgid);
        }
    }

    if args.group_dead != 0 {
        let _ = PID_TIMES.remove(tgid);
        let _ = PID_LAST.remove(tgid);
    }

    Ok(0)
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}

#[unsafe(link_section = "license")]
#[unsafe(no_mangle)]
static LICENSE: [u8; 13] = *b"Dual MIT/GPL\0";
