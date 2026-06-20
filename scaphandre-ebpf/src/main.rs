#![no_std]
#![no_main]

use aya_ebpf::{
    helpers::{bpf_get_current_pid_tgid, bpf_ktime_get_ns, generated::bpf_get_smp_processor_id}, macros::{map, perf_event, tracepoint},
    maps::{Array, PerCpuHashMap, RingBuf},
    programs::{PerfEventContext, TracePointContext}
};
use scaphandre_common::{CpuEventType, CpuStateEvent};

// TODO: Update to proper max keys
const MAX_KEYS: u32 = 1024;
const MAX_CPU: u32 = 128;

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
static CPU_TIME: Array<u64> = Array::with_max_entries(MAX_CPU, 0);

#[map]
static CPU_SNAPSHOT: Array<u64> = Array::with_max_entries(MAX_CPU, 0);

/// Ring buffer for CPU state events consumed by userspace. 256 KB.
#[map]
static CPU_STATE_EVENTS: RingBuf = RingBuf::with_byte_size(256 * 1024, 0);

#[tracepoint]
pub fn scaphandre(ctx: TracePointContext) -> u32 {
    match try_scaphandre(ctx) {
        Ok(ret) => ret,
        Err(ret) => ret,
    }
}

fn try_scaphandre(ctx: TracePointContext) -> Result<u32, u32> {
    let now = unsafe { bpf_ktime_get_ns() };
    let cpu = unsafe { bpf_get_smp_processor_id() };

    let args = unsafe {
        ctx.read_at::<SchedSwitchArgs>(0).map_err(|_| 1u32)?
    };

    let prev_pid = args.prev_pid as u32;
    let next_pid = args.next_pid as u32;

    // Accumulate time for the task that just got switched OFF the CPU
    if let Some(last_timestamp) = PID_LAST.get_ptr(prev_pid) {
        let delta = now - unsafe { *last_timestamp };
        if let Some(p_time) = PID_TIMES.get_ptr_mut(prev_pid) {
            unsafe { *p_time += delta };
        } else {
            let _ = PID_TIMES.insert(prev_pid, delta, 0);
        };
        let _ = PID_LAST.remove(prev_pid);
        if let Some(c_time) = CPU_TIME.get_ptr_mut(cpu) {
            unsafe { *c_time += delta };
        }
    }

    // Record when the incoming task got switched ON
    let _ = PID_LAST.insert(next_pid, now, 0);

    Ok(0)
}

#[perf_event]
pub fn sample_tick(_ctx: PerfEventContext) -> u32 {
    let now = unsafe { bpf_ktime_get_ns() };
    let pid = (bpf_get_current_pid_tgid() & 0xFFFF_FFFF) as u32;
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

/// Fires every 10 ms. TODO: implement idle/active state event emission.
#[perf_event]
pub fn cpu_state_tick(_ctx: PerfEventContext) -> u32 {
    let mut idle = true;
    let mut active_cpus = 0;
    for cpu in 0..MAX_CPU {
        if let Some(old) = CPU_SNAPSHOT.get(cpu) {
            if let Some(new) = CPU_TIME.get(cpu) {
                if new - old != 0 {
                    idle = false;
                    active_cpus += 1;
                }
                if active_cpus >= 2 {
                    break;
                }
            }
        }
    }

    if idle {
        if let Some(mut entry) = CPU_STATE_EVENTS.reserve::<CpuStateEvent>(0) {
            unsafe {
                (*entry.as_mut_ptr()) = CpuStateEvent { 
                    event_type: CpuEventType::IdleEvent,
                };
            }
            entry.submit(0);
        }
    }

    if active_cpus == 1 {
        if let Some(mut entry) = CPU_STATE_EVENTS.reserve::<CpuStateEvent>(0) {
            unsafe {
                (*entry.as_mut_ptr()) = CpuStateEvent { 
                    event_type: CpuEventType::ActivationEvent,
                };
            }
            entry.submit(0);
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
