use aya::{
    maps::{MapData, RingBuf},
    programs::{
        perf_event::{PerfEvent, PerfEventConfig, PerfEventScope, SamplePolicy, SoftwareEvent},
        TracePoint,
    },
    util::online_cpus,
};
use log::warn;
use scaphandre_common::CpuStateEvent;

/// `sample_tick` fires every 5 ms → 200 Hz.
const SAMPLE_TICK_HZ: u64 = 200;

/// `cpu_state_tick` fires every 10 ms → 100 Hz.
const CPU_STATE_TICK_HZ: u64 = 100;

/// Loads the eBPF program and attaches it to sched_switch. Returns the Ebpf handle.
pub fn load() -> anyhow::Result<aya::Ebpf> {
    let rlim = libc::rlimit {
        rlim_cur: libc::RLIM_INFINITY,
        rlim_max: libc::RLIM_INFINITY,
    };
    let ret = unsafe { libc::setrlimit(libc::RLIMIT_MEMLOCK, &rlim) };
    if ret != 0 {
        log::debug!("remove limit on locked memory failed, ret is: {ret}");
    }

    let mut ebpf = aya::Ebpf::load(aya::include_bytes_aligned!(concat!(
        env!("OUT_DIR"),
        "/scaphandre"
    )))?;

    let program: &mut TracePoint = ebpf.program_mut("scaphandre").unwrap().try_into()?;
    program.load()?;
    program.attach("sched", "sched_switch")?;

    let tick: &mut PerfEvent = ebpf.program_mut("sample_tick").unwrap().try_into()?;
    tick.load()?;
    for cpu in online_cpus().map_err(|(_, e)| e)? {
        tick.attach(
            PerfEventConfig::Software(SoftwareEvent::CpuClock),
            PerfEventScope::AllProcessesOneCpu { cpu },
            SamplePolicy::Frequency(SAMPLE_TICK_HZ),
            true,
        )?;
    }
    debug!("Loaded sample_tick eBPF program (200 Hz / 5 ms).");

    let state_tick: &mut PerfEvent = ebpf.program_mut("cpu_state_tick").unwrap().try_into()?;
    state_tick.load()?;
    for cpu in online_cpus().map_err(|(_, e)| e)? {
        state_tick.attach(
            PerfEventConfig::Software(SoftwareEvent::CpuClock),
            PerfEventScope::AllProcessesOneCpu { cpu },
            SamplePolicy::Frequency(CPU_STATE_TICK_HZ),
            true,
        )?;
    }
    debug!("Loaded cpu_state_tick eBPF program (100 Hz / 10 ms).");

    Ok(ebpf)
}

/// Takes the `CPU_STATE_EVENTS` ring buffer out of the eBPF object.
/// Returns `None` if the map is missing or has the wrong type.
pub fn take_cpu_state_buffer(ebpf: &mut aya::Ebpf) -> Option<RingBuf<MapData>> {
    ebpf.take_map("CPU_STATE_EVENTS")
        .and_then(|m| RingBuf::try_from(m).ok())
}

/// Drains all pending [`CpuStateEvent`]s from the ring buffer without blocking.
///
/// # Usage
/// ```rust,ignore
/// if let Some(buf) = &mut topology.cpu_state_buffer {
///     for event in bpf::drain_cpu_state_events(buf) {
///         match event.event_type {
///             CpuEventType::ActivationEvent => { /* cpu became active */ }
///             CpuEventType::IdleEvent       => { /* cpu went idle    */ }
///         }
///     }
/// }
/// ```
pub fn drain_cpu_state_events(buf: &mut RingBuf<MapData>) -> Vec<CpuStateEvent> {
    let mut events = Vec::new();
    while let Some(item) = buf.next() {
        if item.len() == std::mem::size_of::<CpuStateEvent>() {
            // SAFETY: bytes come from the eBPF side as a valid #[repr(C)] CpuStateEvent.
            let event: CpuStateEvent = unsafe {
                std::ptr::read_unaligned(item.as_ptr() as *const CpuStateEvent)
            };
            events.push(event);
        }
    }
    events
}

/// Initialises the eBPF log-flush task on the current tokio runtime.
/// Call this after `load()` from an async context.
pub async fn init_logger(ebpf: &mut aya::Ebpf) {
    match aya_log::EbpfLogger::init(ebpf) {
        Err(e) => {
            warn!("failed to initialize eBPF logger: {e}");
        }
        Ok(logger) => {
            match tokio::io::unix::AsyncFd::with_interest(logger, tokio::io::Interest::READABLE) {
                Err(e) => warn!("failed to create AsyncFd for eBPF logger: {e}"),
                Ok(mut logger) => {
                    tokio::task::spawn(async move {
                        loop {
                            let mut guard = logger.readable_mut().await.unwrap();
                            guard.get_inner_mut().flush();
                            guard.clear_ready();
                        }
                    });
                }
            }
        }
    }
}
