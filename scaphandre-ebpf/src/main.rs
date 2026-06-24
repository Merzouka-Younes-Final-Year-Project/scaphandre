#![no_std]
#![no_main]

use aya_ebpf::{
    helpers::{bpf_get_current_pid_tgid, bpf_ktime_get_ns, generated::bpf_get_smp_processor_id}, macros::{map, perf_event, tracepoint},
    maps::{Array, PerCpuHashMap, RingBuf},
    programs::{PerfEventContext, TracePointContext}
};
use scaphandre_common::{CpuEventType, CpuStateEvent};

// TODO: Ask LLM about fixes from this side and simplify if possible

const MAX_KEYS: u32 = 32768;
const MAX_CPU: u32 = 256;
const MAX_SOCKET: u16 = 64;

/// Layout from /sys/kernel/debug/tracing/events/sched/sched_switch/format
/// prev_state is i64 on kernels >= 5.14, i32 on older ones.
/// Verified for x86_64. Check format file on other architectures.
#[repr(C)]
struct SchedSwitchArgs {
    common_type:     u16,
    common_flags:    u8,
    common_preempt:  u8,
    common_pid:      i32,
    prev_comm:       [u8; 16],
    prev_pid:        i32,
    prev_prio:       i32,
    prev_state:      i64,
    next_comm:       [u8; 16],
    next_pid:        i32,
    next_prio:       i32,
}

#[map]
static PID_TIMES: PerCpuHashMap<u32, u64> = PerCpuHashMap::with_max_entries(MAX_KEYS, 0);

#[map]
static PID_LAST: PerCpuHashMap<u32, u64> = PerCpuHashMap::with_max_entries(MAX_KEYS, 0);

#[map]
static TID_TO_TGID: aya_ebpf::maps::HashMap<u32, u32> = aya_ebpf::maps::HashMap::with_max_entries(MAX_KEYS, 0);

#[map]
static CPU_TIME: Array<u64> = Array::with_max_entries(MAX_CPU, 0);

#[map]
static CPU_SNAPSHOT: Array<u64> = Array::with_max_entries(MAX_CPU, 0);

/// Ring buffer for CPU state events consumed by userspace. 256 KB.
#[map]
static CPU_STATE_EVENTS: RingBuf = RingBuf::with_byte_size(256 * 1024, 0);

#[map]
static CPU_TO_SOCKET: Array<u16> = Array::with_max_entries(MAX_CPU, 0);

#[tracepoint]
pub fn scaphandre(ctx: TracePointContext) -> u32 {
    match try_sched_switch(ctx) {
        Ok(ret) => ret,
        Err(ret) => ret,
    }
}

// AI: This should be renamed properly for what it does and synced with userspace code
fn try_sched_switch(ctx: TracePointContext) -> Result<u32, u32> {
    let now = unsafe { bpf_ktime_get_ns() };
    let cpu = unsafe { bpf_get_smp_processor_id() };

    let args = unsafe {
        ctx.read_at::<SchedSwitchArgs>(0).map_err(|_| 1u32)?
    };

    // Use TGID (upper 32 bits of pid_tgid) so keys match userspace process PIDs.
    // For next_pid we can read TGID directly; for prev we derive it from the tid via a lookup.
    let prev_tid = args.prev_pid as u32;
    let next_tgid = (bpf_get_current_pid_tgid() >> 32) as u32;
    // Prev TGID: look up from our tid→tgid map seeded when tasks are scheduled in.
    let prev_tgid = unsafe {
        TID_TO_TGID.get(prev_tid).copied().unwrap_or(prev_tid)
    };

    // Seed tid→tgid for the incoming task so future prev lookups work.
    let _ = TID_TO_TGID.insert(args.next_pid as u32, next_tgid, 0);

    // Accumulate time for the task that just got switched OFF the CPU
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

    // Record when the incoming task got switched ON
    let _ = PID_LAST.insert(next_tgid, now, 0);

    Ok(0)
}

#[perf_event]
pub fn sample_tick(_ctx: PerfEventContext) -> u32 {
    let now = unsafe { bpf_ktime_get_ns() };
    let pid = (bpf_get_current_pid_tgid() >> 32) as u32;
    let cpu = unsafe { bpf_get_smp_processor_id() };

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
// AI: Fix this to execute only on one core per socket. This should be done in userspace by
// retrieving sockets and hooking into the first core
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

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}

#[unsafe(link_section = "license")]
#[unsafe(no_mangle)]
static LICENSE: [u8; 13] = *b"Dual MIT/GPL\0";
