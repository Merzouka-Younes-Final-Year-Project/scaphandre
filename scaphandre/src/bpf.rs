use aya::programs::TracePoint;
use log::warn;

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

    Ok(ebpf)
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
