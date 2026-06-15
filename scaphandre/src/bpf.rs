use aya::{maps::PerCpuHashMap, programs::TracePoint};
#[rustfmt::skip]
use log::{debug, warn};
use tokio::io::{AsyncBufReadExt, BufReader, AsyncWriteExt};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();

    let rlim = libc::rlimit {
        rlim_cur: libc::RLIM_INFINITY,
        rlim_max: libc::RLIM_INFINITY,
    };
    let ret = unsafe { libc::setrlimit(libc::RLIMIT_MEMLOCK, &rlim) };
    if ret != 0 {
        debug!("remove limit on locked memory failed, ret is: {ret}");
    }

    let mut ebpf = aya::Ebpf::load(aya::include_bytes_aligned!(concat!(
        env!("OUT_DIR"),
        "/scaphandre"
    )))?;

    match aya_log::EbpfLogger::init(&mut ebpf) {
        Err(e) => {
            warn!("failed to initialize eBPF logger: {e}");
        }
        Ok(logger) => {
            let mut logger =
                tokio::io::unix::AsyncFd::with_interest(logger, tokio::io::Interest::READABLE)?;
            tokio::task::spawn(async move {
                loop {
                    let mut guard = logger.readable_mut().await.unwrap();
                    guard.get_inner_mut().flush();
                    guard.clear_ready();
                }
            });
        }
    }

    let program: &mut TracePoint = ebpf.program_mut("scaphandre").unwrap().try_into()?;
    program.load()?;
    program.attach("sched", "sched_switch")?;

    let map = PerCpuHashMap::<_, u32, u64>::try_from(
        ebpf.map("PID_TIMES").unwrap()
    )?;



    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();
    let mut stdout = tokio::io::stdout();

    println!("Enter '<pid> <core>' to query, 'q' to quit.");

    loop {
        stdout.write_all(b"> ").await?;
        stdout.flush().await?;

        let line = match lines.next_line().await? {
            Some(l) => l,
            None => break,
        };

        let line = line.trim();

        if line == "q" {
            println!("Exiting...");
            break;
        }

        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() != 2 {
            println!("Usage: <pid> <core>");
            continue;
        }

        let (Ok(pid), Ok(core)) = (parts[0].parse::<u32>(), parts[1].parse::<usize>()) else {
            println!("Invalid pid or core number.");
            continue;
        };

        match map.get(&pid, 0) {
            Ok(per_cpu) => match per_cpu.get(core) {
                Some(&ns) => println!("PID {} CPU{}: {} ms", pid, core, ns / 1_000_000),
                None => println!("Core {} does not exist.", core),
            },
            Err(_) => println!("PID {} not found.", pid),
        }
    }

    Ok(())
}
