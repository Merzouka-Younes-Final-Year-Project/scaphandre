#![no_std]
#![no_main]

use aya_ebpf::{
    EbpfContext,
    macros::{map, tracepoint},
    maps::PerCpuHashMap,
    programs::TracePointContext,
    helpers::bpf_ktime_get_ns,
};

const MAX_KEYS: u32 = 1024;

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

#[tracepoint]
pub fn scaphandre(ctx: TracePointContext) -> u32 {
    match try_scaphandre(ctx) {
        Ok(ret) => ret,
        Err(ret) => ret,
    }
}

fn try_scaphandre(ctx: TracePointContext) -> Result<u32, u32> {
    let now = unsafe { bpf_ktime_get_ns() };

    let args = unsafe {
        ctx.read_at::<SchedSwitchArgs>(0).map_err(|_| 1u32)?
    };

    let prev_pid = args.prev_pid as u32;
    let next_pid = args.next_pid as u32;

    // Accumulate time for the task that just got switched OFF the CPU
    if let Some(last_ptr) = PID_LAST.get_ptr(prev_pid) {
        let delta = now - unsafe { *last_ptr };
        if let Some(p_time) = PID_TIMES.get_ptr_mut(prev_pid) {
            unsafe { *p_time += delta };
        } else {
            let _ = PID_TIMES.insert(prev_pid, delta, 0);
        }
    }

    // Record when the incoming task got switched ON
    let _ = PID_LAST.insert(next_pid, now, 0);

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
