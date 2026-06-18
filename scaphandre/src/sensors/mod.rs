//! # Sensors: to get data related to energy consumption
//!
//! `Sensor` is the root for all sensors. It defines the [Sensor] trait
//! needed to implement a sensor.

#[cfg(target_os = "windows")]
pub mod msr_rapl;
use aya::Ebpf;
use docker_sync::network;
#[cfg(target_os = "windows")]
use msr_rapl::get_msr_value;
#[cfg(target_os = "linux")]
pub mod powercap_rapl;
pub mod units;
pub mod utils;
#[cfg(target_os = "linux")]
use procfs::{CpuInfo, CpuTime, KernelStats};
use std::cell::RefCell;
use std::{collections::HashMap, error::Error, fmt, fs, mem::size_of_val, str, time::Duration, u64, vec};
#[cfg(target_os = "linux")]
use std::os::unix::fs::FileExt;
#[allow(unused_imports)]
use sysinfo::{CpuExt, Pid, System, SystemExt};
use sysinfo::{DiskExt, DiskType, ProcessExt};
use utils::{current_system_time_since_epoch, IProcess, ProcessTracker};
use std::cmp::min;
use perf_event::{Builder, Counter};
use perf_event::events::Hardware;

// !!!!!!!!!!!!!!!!! Sensor !!!!!!!!!!!!!!!!!!!!!!!
/// Sensor trait, the Sensor API.
pub trait Sensor {
    fn get_topology(&self) -> Box<Option<Topology>>;
    fn generate_topology(&self) -> Result<Topology, Box<dyn Error>>;
}

/// Defines methods for Record instances creation
/// and storage.
pub trait RecordGenerator {
    fn refresh_record(&mut self);
    fn get_records_passive(&self) -> Vec<Record>;
    fn clean_old_records(&mut self);
}

pub trait MultiValuedRecordGenerator {
    fn refresh_record(&mut self);
    fn get_records_passive(&self) -> Vec<MultiValuedRecord>;
    fn clean_old_records(&mut self);
}

pub trait MultiValuedRecordReader {
    fn read_record(&self) -> Result<MultiValuedRecord, Box<dyn Error>>;
}

pub trait RecordReader {
    fn read_record(&self) -> Result<Record, Box<dyn Error>>;
}

// !!!!!!!!!!!!!!!!! Topology !!!!!!!!!!!!!!!!!!!!!!!
/// Topology struct represents the whole CPUSocket architecture,
/// from the electricity consumption point of view,
/// including the potentially multiple CPUSocket sockets.
/// Owns a vector of CPUSocket structs representing each socket.
#[derive(Debug)]
pub struct Topology {
    /// The CPU sockets found on the host, represented as CPUSocket instances attached to this topology
    pub sockets: Vec<CPUSocket>,
    /// ProcessTrack instance that keeps track of processes running on the host and CPU stats associated
    pub proc_tracker: ProcessTracker,
    /// CPU usage stats buffer
    pub stat_buffer: Vec<CPUStat>,
    /// Measurements of energy usage, stored as Record instances
    pub record_buffer: Vec<Record>,
    /// Maximum size in memory for the recor_buffer
    pub buffer_max_kbytes: u16,
    /// Sorted list of all domains names
    pub domains_names: Option<Vec<String>>,
    /// Sensor-specific data needed in the topology
    pub _sensor_data: HashMap<String, String>,
    /// Ebpf Program Handle
    pub ebpf: Option<Ebpf>,
}

impl RecordGenerator for Topology {
    /// Computes a new Record, stores it in the record_buffer
    /// and returns a clone of this record.
    ///
    fn refresh_record(&mut self) {
        match self.read_record() {
            Ok(record) => {
                self.record_buffer.push(record);
            }
            Err(e) => {
                warn!(
                    "Could'nt read record from {}, error was : {:?}",
                    self._sensor_data
                        .get("source_file")
                        .unwrap_or(&String::from("SRCFILENOTKNOWN")),
                    e
                );
            }
        }

        if !self.record_buffer.is_empty() {
            self.clean_old_records();
        }
    }

    /// Removes (and thus drops) as many Record instances from the record_buffer
    /// as needed for record_buffer to not exceed 'buffer_max_kbytes'
    fn clean_old_records(&mut self) {
        let record_ptr = &self.record_buffer[0];
        let record_size = size_of_val(record_ptr);
        let curr_size = record_size * self.record_buffer.len();
        trace!(
            "topology: current size of record buffer: {} max size: {}",
            curr_size,
            self.buffer_max_kbytes * 1000
        );
        if curr_size as u16 > self.buffer_max_kbytes * 1000 {
            let size_diff = curr_size - (self.buffer_max_kbytes * 1000) as usize;
            trace!(
                "topology: size_diff: {} record size: {}",
                size_diff,
                record_size
            );
            if size_diff > record_size {
                let nb_records_to_delete = size_diff as f32 / record_size as f32;
                for _ in 1..nb_records_to_delete as u32 {
                    if !self.record_buffer.is_empty() {
                        let res = self.record_buffer.remove(0);
                        debug!("Cleaning record buffer on Topology, removing: {:?}", res);
                    }
                }
            }
        }
    }

    /// Returns a copy of the record_buffer
    fn get_records_passive(&self) -> Vec<Record> {
        let mut result = vec![];
        for r in &self.record_buffer {
            result.push(Record::new(
                r.timestamp,
                r.value.clone(),
                units::Unit::MicroJoule,
            ));
        }
        result
    }
}

impl Default for Topology {
    fn default() -> Self {
        {
            Self::new(HashMap::new())
        }
    }
}

impl Clone for Topology {
    fn clone(&self) -> Self {
        Topology {
            sockets: self.sockets.clone(),
            proc_tracker: self.proc_tracker.clone(),
            stat_buffer: self.stat_buffer.clone(),
            record_buffer: self.record_buffer.clone(),
            buffer_max_kbytes: self.buffer_max_kbytes,
            domains_names: self.domains_names.clone(),
            _sensor_data: self._sensor_data.clone(),
            ebpf: None, // Ebpf handles cannot be cloned; cloned topologies won't track eBPF data
        }
    }
}

impl Topology {
    /// Instanciates Topology and returns the instance
    pub fn new(sensor_data: HashMap<String, String>) -> Topology {
        let mut ebpf = crate::bpf::load().ok();
        Topology {
            sockets: vec![],
            proc_tracker: ProcessTracker::new(5, ebpf.as_mut()),
            stat_buffer: vec![],
            record_buffer: vec![],
            buffer_max_kbytes: 1,
            domains_names: None,
            _sensor_data: sensor_data,
            ebpf,
        }
    }

    /// Parses /proc/cpuinfo and creates instances of CPUCore.
    ///
    ///# Examples
    ///
    /// ```
    /// use scaphandre::sensors::Topology;
    ///
    /// if let Some(cores) = Topology::generate_cpu_cores() {
    ///     println!("There are {} cores on this host.", cores.len());
    ///     for c in &cores {
    ///         println!("CPU info {:?}", c.attributes);
    ///     }
    /// }
    /// ```
    pub fn generate_cpu_cores() -> Option<Vec<CPUCore>> {
        let mut cores = vec![];

        let sysinfo_system = System::new_all();
        let sysinfo_cores = sysinfo_system.cpus();
        warn!("Sysinfo sees {}", sysinfo_cores.len());
        #[cfg(target_os = "linux")]
        let cpuinfo = CpuInfo::new().unwrap();
        for (idx, c) in (0_u16..).zip(sysinfo_cores.iter()) {
            let id: u16 = c.name()
                .trim_start_matches(|ch: char| !ch.is_ascii_digit())
                .parse()
                .unwrap_or(idx);
            let mut info = HashMap::<String, String>::new();
            #[cfg(target_os = "linux")]
            {
                for (k, v) in cpuinfo.get_info(id as usize).unwrap().iter() {
                    info.insert(String::from(*k), String::from(*v));
                }
            }
            info.insert(String::from("frequency"), c.frequency().to_string());
            info.insert(String::from("name"), c.name().to_string());
            info.insert(String::from("vendor_id"), c.vendor_id().to_string());
            info.insert(String::from("brand"), c.brand().to_string());
            cores.push(CPUCore::new(id, info));
        }
        Some(cores)
    }

    /// Adds a Socket instance to self.sockets if and only if the
    /// socket id doesn't exist already.
    pub fn safe_add_socket(
        &mut self,
        socket_id: u16,
        domains: Vec<Domain>,
        attributes: Vec<Vec<HashMap<String, String>>>,
        counter_uj_path: String,
        buffer_max_kbytes: u16,
        sensor_data: HashMap<String, String>,
    ) -> Option<CPUSocket> {
        if !self.sockets.iter().any(|s| s.id == socket_id) {
            let socket = CPUSocket::new(
                socket_id,
                domains,
                attributes,
                counter_uj_path,
                buffer_max_kbytes,
                sensor_data,
            );
            let res = socket.clone();
            self.sockets.push(socket);
            Some(res)
        } else {
            None
        }
    }

    pub fn safe_insert_socket(&mut self, socket: CPUSocket) {
        if !self.sockets.iter().any(|s| s.id == socket.id) {
            self.sockets.push(socket);
        }
    }

    /// Returns a immutable reference to self.proc_tracker
    pub fn get_proc_tracker(&self) -> &ProcessTracker {
        &self.proc_tracker
    }

    /// Returns a mutable reference to self.sockets
    pub fn get_sockets(&mut self) -> &mut Vec<CPUSocket> {
        &mut self.sockets
    }

    /// Returns an immutable reference to self.sockets
    pub fn get_sockets_passive(&self) -> &Vec<CPUSocket> {
        &self.sockets
    }

    // Build a sorted list of all domains names from all sockets.
    fn build_domains_names(&mut self) {
        let mut names: HashMap<String, ()> = HashMap::new();
        for s in self.sockets.iter() {
            for d in s.get_domains_passive() {
                names.insert(d.name.clone(), ());
            }
        }
        let mut domain_names = names.keys().cloned().collect::<Vec<String>>();
        domain_names.sort();
        self.domains_names = Some(domain_names);
    }

    pub fn set_domains_names(&mut self, names: Vec<String>) {
        self.domains_names = Some(names);
    }

    /// Adds a Domain instance to a given socket, if and only if the domain
    /// id doesn't exist already for the socket.
    pub fn safe_add_domain_to_socket(
        &mut self,
        socket_id: u16,
        domain_id: u16,
        name: &str,
        uj_counter: &str,
        buffer_max_kbytes: u16,
        sensor_data: HashMap<String, String>,
    ) {
        let iterator = self.sockets.iter_mut();
        for socket in iterator {
            if socket.id == socket_id {
                socket.safe_add_domain(Domain::new(
                    domain_id,
                    String::from(name),
                    String::from(uj_counter),
                    buffer_max_kbytes,
                    sensor_data.clone(),
                ));
            }
        }
        self.build_domains_names();
    }

    /// Generates CPUCore instances for the host and adds them
    /// to appropriate CPUSocket instance from self.sockets
    #[cfg(target_os = "linux")]
    pub fn add_cpu_cores(&mut self) {
        if let Some(mut cores) = Topology::generate_cpu_cores() {
            while let Some(c) = cores.pop() {
                let socket_id = &c
                    .attributes
                    .get("physical id")
                    .unwrap()
                    .parse::<u16>()
                    .unwrap();
                let socket_match = self.sockets.iter_mut().find(|x| &x.id == socket_id);

                //In VMs there might be a missmatch betwen Sockets and Cores - see Issue#133 as a first fix we just map all cores that can't be mapped to the first
                let socket = match socket_match {
                    Some(x) => x,
                    None =>self.sockets.first_mut().expect("Trick: if you are running on a vm, do not forget to use --vm parameter invoking scaphandre at the command line")
                };

                if socket_id == &socket.id {
                    socket.add_cpu_core(c);
                } else {
                    socket.add_cpu_core(c);
                    warn!("coud't not match core to socket - mapping to first socket instead - if you are not using --vm there is something wrong")
                }
            }
        } else {
            panic!("Couldn't retrieve any CPU Core from the topology. (generate_cpu_cores)");
        }
    }

    /// Refresh cpuidle idle time records for all cores in all sockets.
    fn refresh_core_idle_records(&mut self) {
        for socket in &mut self.sockets {
            for core in socket.get_cores() {
                core.refresh_record();
            }
        }
    }
}

// Placeholder for now to avoid duplicate code structure issues
#[cfg(not(target_os = "linux"))]
impl Topology {
    fn refresh_core_idle_records(&mut self) {}
}

#[cfg(target_os = "linux")]
impl Topology {
    // refresh_core_idle_records is implemented above
}

// Re-open the impl block to add remaining methods
impl Topology {
    /// (Existing methods continue here)
            //#[cfg(target_os = "windows")]
            //{{
            //TODO: fix
            //let nb_sockets = &self.sockets.len();
            //let mut socket_counter = 0;
            //let nb_cores_per_socket = &cores.len() / nb_sockets;
            //warn!("nb_cores_per_socket: {} cores_len: {} sockets_len: {}", nb_cores_per_socket, &cores.len(), &self.sockets.len());
            //for s in self.sockets.iter_mut() {
            //    for c in (socket_counter * nb_cores_per_socket)..((socket_counter+1) * nb_cores_per_socket) {
            //        match cores.pop() {
            //            Some(core) => {
            //                warn!("adding core {} to socket {}", core.id, s.id);
            //                s.add_cpu_core(core);
            //            },
            //            None => {
            //                error!("Uneven number of CPU cores !");
            //            }
            //        }
            //    }
            //    socket_counter = socket_counter + 1;
            //}
            //}
    //

    /// Triggers ProcessTracker refresh on process stats
    /// and power consumption, CPU stats and cores power comsumption,
    /// CPU sockets stats and power consumption.
    pub fn refresh(&mut self) {
        let sockets = &mut self.sockets;
        for s in sockets {
            // refresh each socket with new record
            s.refresh_record();
            s.refresh_stats();
            let domains = s.get_domains();
            for d in domains {
                d.refresh_record();
            }
            //let cores = s.get_cores();
            //for c in cores {
            //
            //}
        }
        self.proc_tracker.refresh();
        self.refresh_core_idle_records();
        self.refresh_procs();
        self.refresh_record();
        self.refresh_stats();
    }

    /// Gets currently running processes (as procfs::Process instances) and stores
    /// them in self.proc_tracker
    fn refresh_procs(&mut self) {
        {
            let pt = &mut self.proc_tracker;
            pt.sysinfo.refresh_processes();
            let current_procs = pt
                .sysinfo
                .processes()
                .values()
                .map(|p| IProcess::new(p, pt.get_process_core_times(p.pid())))
                .collect::<Vec<_>>();
            for p in current_procs {
                match pt.add_process_record(p) {
                    Ok(_) => {}
                    Err(msg) => {
                        panic!("Failed to track process !\nGot: {}", msg)
                    }
                }
            }
        }
    }

    /// Gets currents stats and stores them as a CPUStat instance in self.stat_buffer
    pub fn refresh_stats(&mut self) {
        if let Some(stats) = self.read_stats() {
            self.stat_buffer.insert(0, stats);
            if !self.stat_buffer.is_empty() {
                self.clean_old_stats();
            }
        } else {
            debug!("read_stats() is None");
        }
    }

    /// Checks the size in memory of stats_buffer and deletes as many CPUStat
    /// instances from the buffer to make it smaller in memory than buffer_max_kbytes.
    fn clean_old_stats(&mut self) {
        let stat_ptr = &self.stat_buffer[0];
        let size_of_stat = size_of_val(stat_ptr);
        let curr_size = size_of_stat * self.stat_buffer.len();
        trace!("current_size of stats in topo: {}", curr_size);
        if curr_size > (self.buffer_max_kbytes * 1000) as usize {
            let size_diff = curr_size - (self.buffer_max_kbytes * 1000) as usize;
            if size_diff > size_of_stat {
                let nb_stats_to_delete = size_diff as f32 / size_of_stat as f32;
                trace!(
                    "nb_stats_to_delete: {} size_diff: {} size of: {}",
                    nb_stats_to_delete,
                    size_diff,
                    size_of_stat
                );
                for _ in 1..nb_stats_to_delete as u32 {
                    if !self.stat_buffer.is_empty() {
                        let res = self.stat_buffer.pop();
                        debug!("Cleaning topology stat buffer, removing: {:?}", res);
                    }
                }
            }
        }
    }

    pub fn get_idle_power_microwatts(&self) -> Option<Record> {
        let mut total = 0_u64;
        for s in &self.sockets {
            if let Some(Ok(idle)) = s.get_idle_power_microwatts().map(|r| r.value.parse::<u64>()) {
                total += idle;
            }
        }
        debug!("Topology IDLE: {total}");

        Some(Record::new(
            current_system_time_since_epoch(),
            total.to_string(),
            units::Unit::MicroWatt,
        ))
    }

    /// Returns a Record instance containing the difference (attribute by attribute, except timestamp which will be the timestamp from the last record)
    /// between the last (in time) record from self.record_buffer and the previous one
    pub fn get_records_diff(&self) -> Option<Record> {
        let len = self.record_buffer.len();
        if len > 2 {
            let last = self.record_buffer.last().unwrap();
            let previous = self.record_buffer.get(len - 2).unwrap();
            let last_value = last.value.parse::<u64>().unwrap();
            let previous_value = previous.value.parse::<u64>().unwrap();
            if previous_value <= last_value {
                let diff = last_value - previous_value;
                return Some(Record::new(last.timestamp, diff.to_string(), last.unit));
            }
        }
        None
    }

    /// Returns the set of cores in all sockets
    pub fn get_cores(&self) -> Vec<CPUCore> {
        let mut cores = self.sockets
            .iter()
            .flat_map(|s| s.cpu_cores.iter().cloned())
            .collect::<Vec<CPUCore>>();
        cores.sort_by_key(|c| c.id);
        cores
    }
    
    /// Returns a MultiValuedRecord instance containing the per-core power consumed between
    /// last and previous measurement, in microwatts.
    pub fn get_core_diff_power_microwatts(&self) -> Option<MultiValuedRecord> {
        let conso = self.get_records_diff_power_microwatts()
            .and_then(|r| r.value.parse::<f64>().ok()).unwrap_or(0_f64);
        debug!("Using formula v_active_aperf");
        let coefs = self.get_cores()
            .iter()
            .map(|c| {
                if let Some(metrics) = c.get_core_metrics_delta() {
                  (1_f64 + metrics.active_percentage) * metrics.aperf as f64
                } else {
                    0_f64
                }
            }).collect::<Vec<f64>>();
        let total_coefs: f64 = coefs.iter().sum();
        let coefs = coefs
            .iter()
            .map(|c| if total_coefs != 0_f64 { c/total_coefs } else { 0_f64 } * conso)
            .collect::<Vec<f64>>();
        Some(MultiValuedRecord::new(
            current_system_time_since_epoch(),
            coefs.iter().map(|c| c.to_string()).collect(),
            coefs.iter().map(|_| units::Unit::Numeric).collect(),
        ))
    }

    /// Returns a Record instance containing the power consumed between
    /// last and previous measurement, in microwatts.
    pub fn get_records_diff_power_microwatts(&self) -> Option<Record> {
        if self.record_buffer.len() > 1 {
            let idle_conso = self
                .get_idle_power_microwatts()
                .map(|r| r.value.parse::<u64>())
                .unwrap_or(Ok(0))
                .unwrap_or(0);

            debug!("Total IDLE power Diff: {idle_conso}");

            let last_record = self.record_buffer.last().unwrap();
            let previous_record = self
                .record_buffer
                .get(self.record_buffer.len() - 2)
                .unwrap();
            match previous_record.value.trim().parse::<u128>() {
                Ok(previous_microjoules) => match last_record.value.trim().parse::<u128>() {
                    Ok(last_microjoules) => {
                        if previous_microjoules > last_microjoules {
                            return None;
                        }
                        let microjoules = last_microjoules - previous_microjoules;
                        let time_diff = last_record.timestamp.as_secs_f64()
                            - previous_record.timestamp.as_secs_f64();
                        let microwatts = microjoules as f64 / time_diff;
                        return Some(Record::new(
                            last_record.timestamp,
                            (microwatts as u64).saturating_sub(idle_conso).to_string(),
                            units::Unit::MicroWatt,
                        ));
                    }
                    Err(e) => {
                        warn!(
                            "Could'nt get previous_microjoules - value : '{}' - error : {:?}",
                            previous_record.value, e
                        );
                    }
                },
                Err(e) => {
                    warn!(
                        "Couldn't parse previous_microjoules - value : '{}' - error : {:?}",
                        previous_record.value.trim(),
                        e
                    );
                }
            }
        }
        None
    }

    /// Returns a CPUStat instance containing the difference between last
    /// and previous stats measurement (from stat_buffer), attribute by attribute.
    pub fn get_stats_diff(&self) -> Option<CPUStat> {
        if self.stat_buffer.len() > 1 {
            let last = &self.stat_buffer[0];
            let previous = &self.stat_buffer[1];
            let mut iowait = None;
            let mut irq = None;
            let mut softirq = None;
            let mut steal = None;
            let mut guest = None;
            let mut guest_nice = None;
            if last.iowait.is_some() && previous.iowait.is_some() {
                iowait = Some(last.iowait.unwrap() - previous.iowait.unwrap());
            }
            if last.irq.is_some() && previous.irq.is_some() {
                irq = Some(last.irq.unwrap() - previous.irq.unwrap());
            }
            if last.softirq.is_some() && previous.softirq.is_some() {
                softirq = Some(last.softirq.unwrap() - previous.softirq.unwrap());
            }
            if last.steal.is_some() && previous.steal.is_some() {
                steal = Some(last.steal.unwrap() - previous.steal.unwrap());
            }
            if last.guest.is_some() && previous.guest.is_some() {
                guest = Some(last.guest.unwrap() - previous.guest.unwrap());
            }
            if last.guest_nice.is_some() && previous.guest_nice.is_some() {
                guest_nice = Some(last.guest_nice.unwrap() - previous.guest_nice.unwrap());
            }
            return Some(CPUStat {
                user: last.user - previous.user,
                nice: last.nice - previous.nice,
                system: last.system - previous.system,
                idle: last.idle - previous.idle,
                iowait,
                irq,
                softirq,
                steal,
                guest,
                guest_nice,
            });
        }
        None
    }

    /// Reads content from /proc/stat and extracts the stats of the whole CPU topology.
    pub fn read_stats(&self) -> Option<CPUStat> {
        #[cfg(target_os = "linux")]
        {
            let kernelstats_or_not = KernelStats::new();
            if let Ok(res_cputime) = kernelstats_or_not {
                return Some(CPUStat {
                    user: res_cputime.total.user,
                    guest: res_cputime.total.guest,
                    guest_nice: res_cputime.total.guest_nice,
                    idle: res_cputime.total.idle,
                    iowait: res_cputime.total.iowait,
                    irq: res_cputime.total.irq,
                    nice: res_cputime.total.nice,
                    softirq: res_cputime.total.softirq,
                    steal: res_cputime.total.steal,
                    system: res_cputime.total.system,
                });
            }
        }
        None
    }

    /// Returns the number of processes currently available
    pub fn read_nb_process_total_count(&self) -> Option<u64> {
        #[cfg(target_os = "linux")]
        {
            if let Ok(result) = KernelStats::new() {
                return Some(result.processes);
            }
        }
        None
    }

    /// Returns the number of processes currently in a running state
    pub fn read_nb_process_running_current(&self) -> Option<u32> {
        #[cfg(target_os = "linux")]
        {
            if let Ok(result) = KernelStats::new() {
                if let Some(procs_running) = result.procs_running {
                    return Some(procs_running);
                }
            }
        }
        None
    }
    /// Returns the number of processes currently blocked waiting
    pub fn read_nb_process_blocked_current(&self) -> Option<u32> {
        #[cfg(target_os = "linux")]
        {
            if let Ok(result) = KernelStats::new() {
                if let Some(procs_blocked) = result.procs_blocked {
                    return Some(procs_blocked);
                }
            }
        }
        None
    }
    /// Returns the current number of context switches
    pub fn read_nb_context_switches_total_count(&self) -> Option<u64> {
        #[cfg(target_os = "linux")]
        {
            if let Ok(result) = KernelStats::new() {
                return Some(result.ctxt);
            }
        }
        None
    }

    pub fn get_cpu_frequency(&self) -> Record {
        Record::new(
            current_system_time_since_epoch(),
            self.proc_tracker.get_cpu_frequency().to_string(),
            units::Unit::MegaHertz,
        )
    }

    pub fn get_load_avg(&self) -> Option<Vec<Record>> {
        let load = self.get_proc_tracker().sysinfo.load_average();
        let timestamp = current_system_time_since_epoch();
        Some(vec![
            Record::new(timestamp, load.one.to_string(), units::Unit::Numeric),
            Record::new(timestamp, load.five.to_string(), units::Unit::Numeric),
            Record::new(timestamp, load.five.to_string(), units::Unit::Numeric),
        ])
    }

    pub fn get_disks(&self) -> HashMap<String, (String, HashMap<String, String>, Record)> {
        let timestamp = current_system_time_since_epoch();
        let mut res = HashMap::new();
        for d in self.proc_tracker.sysinfo.disks() {
            let mut attributes = HashMap::new();
            if let Ok(file_system) = str::from_utf8(d.file_system()) {
                attributes.insert(String::from("disk_file_system"), String::from(file_system));
            }
            if let Some(mount_point) = d.mount_point().to_str() {
                attributes.insert(String::from("disk_mount_point"), String::from(mount_point));
            }
            match d.type_() {
                DiskType::SSD => {
                    attributes.insert(String::from("disk_type"), String::from("SSD"));
                }
                DiskType::HDD => {
                    attributes.insert(String::from("disk_type"), String::from("HDD"));
                }
                DiskType::Unknown(_) => {
                    attributes.insert(String::from("disk_type"), String::from("Unknown"));
                }
            }
            attributes.insert(
                String::from("disk_is_removable"),
                d.is_removable().to_string(),
            );
            if let Some(disk_name) = d.name().to_str() {
                attributes.insert(String::from("disk_name"), String::from(disk_name));
            }
            res.insert(
                String::from("scaph_host_disk_total_bytes"),
                (
                    String::from("Total disk size, in bytes."),
                    attributes.clone(),
                    Record::new(timestamp, d.total_space().to_string(), units::Unit::Bytes),
                ),
            );
            res.insert(
                String::from("scaph_host_disk_available_bytes"),
                (
                    String::from("Available disk space, in bytes."),
                    attributes.clone(),
                    Record::new(
                        timestamp,
                        d.available_space().to_string(),
                        units::Unit::Bytes,
                    ),
                ),
            );
        }
        res
    }

    pub fn get_total_memory_bytes(&self) -> Record {
        Record {
            timestamp: current_system_time_since_epoch(),
            value: self.proc_tracker.sysinfo.total_memory().to_string(),
            unit: units::Unit::Bytes,
        }
    }

    pub fn get_available_memory_bytes(&self) -> Record {
        Record {
            timestamp: current_system_time_since_epoch(),
            value: self.proc_tracker.sysinfo.available_memory().to_string(),
            unit: units::Unit::Bytes,
        }
    }

    pub fn get_free_memory_bytes(&self) -> Record {
        Record {
            timestamp: current_system_time_since_epoch(),
            value: self.proc_tracker.sysinfo.free_memory().to_string(),
            unit: units::Unit::Bytes,
        }
    }

    pub fn get_total_swap_bytes(&self) -> Record {
        Record {
            timestamp: current_system_time_since_epoch(),
            value: self.proc_tracker.sysinfo.total_swap().to_string(),
            unit: units::Unit::Bytes,
        }
    }

    pub fn get_free_swap_bytes(&self) -> Record {
        Record {
            timestamp: current_system_time_since_epoch(),
            value: self.proc_tracker.sysinfo.free_swap().to_string(),
            unit: units::Unit::Bytes,
        }
    }

    pub fn get_process_power_consumption_microwatts(&self, pid: Pid) -> Option<Record> {
        if let Some(record) = self.get_proc_tracker().get_process_last_record(pid) {
            let process_cpu_percentage = self.get_process_cpu_usage_percentage(pid).unwrap();
            let topo_conso = self.get_records_diff_power_microwatts();
            if let Some(conso) = &topo_conso {
                let conso_f64 = conso.value.parse::<f64>().unwrap();
                let result =
                    (conso_f64 * process_cpu_percentage.value.parse::<f64>().unwrap()) / 100.0_f64;
                return Some(Record::new(
                    record.timestamp,
                    result.to_string(),
                    units::Unit::MicroWatt,
                ));
            }
        } else {
            trace!("Couldn't find records for PID: {}", pid);
        }
        None
    }

    /// NOTE: TO MODIFY
    /// Returns the power consumed between last and previous measurement for a given process ID, in microwatts
    pub fn get_all_per_process(&self, pid: Pid) -> Option<HashMap<String, (String, Record)>> {
        let mut res = HashMap::new();
        if let Some(record) = self.get_proc_tracker().get_process_last_record(pid) {
            let mut core_percentages: Option<Vec<f64>> = None;
            let cores: Vec<CPUCore> = self.get_cores();
            let cores_metrics: Vec<Option<CPUCoreMetrics>> = cores
                .iter()
                .map(|c| c.get_core_metrics_delta())
                .collect();
            if let Some(core_time_deltas) = self.get_proc_tracker().get_per_core_cpu_time_delta(pid) {
                debug!(
                    "Gotten EBPF per core times process {pid}: {}, Core IDs: {}",
                    core_time_deltas.iter().map(|v| v.to_string()).collect::<Vec<String>>().join(", "),
                    cores.iter().map(|c| c.id.to_string()).collect::<Vec<String>>().join(", "),
                );
                core_percentages = Some(
                    cores.iter().enumerate().map(|t| {
                        if let Some(core_metrics) = &cores_metrics[t.0] {
                            if core_metrics.active_time != 0 {
                                let ticks_per_sec = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as u64;
                                let active_time_ns = core_metrics.active_time * 1_000_000_000 / ticks_per_sec;
                                core_time_deltas[t.1.id as usize] as f64 / active_time_ns as f64
                            } else {
                                0_f64
                            }
                        } else {
                            0.0_f64
                        }
                    }).collect()
                )
            }
            if let Some(process_cpu_percentage) = self.get_proc_tracker().get_process_cpu_time_delta_as_percentage(pid) {
                if core_percentages.is_none() {
                    debug!("Falling back to standard non-EBPF method");
                    core_percentages = Some(
                        cores_metrics
                            .iter()
                            .map(|o| {
                                o.as_ref().map(|metrics| {
                                    (process_cpu_percentage / 100.0_f64) * (metrics.cpu_time_percentage / 100.0_f64)
                                }).unwrap_or(0.0_f64)
                            }).collect()
                    );
                }
                res.insert(
                    String::from("scaph_process_cpu_usage_percentage"),
                    (String::from("CPU time consumed by the process, as a percentage of the capacity of all the CPU Cores"),
                        Record::new(
                            record.timestamp,
                            process_cpu_percentage.to_string(),
                            units::Unit::Percentage,
                        )
                    )
                );
                res.insert(
                    String::from("scaph_process_memory_virtual_bytes"),
                    (
                        String::from("Virtual RAM usage by the process, in bytes"),
                        Record::new(
                            record.timestamp,
                            record.process.virtual_memory.to_string(),
                            units::Unit::Percentage,
                        ),
                    ),
                );
                res.insert(
                    String::from("scaph_process_memory_bytes"),
                    (
                        String::from("Physical RAM usage by the process, in bytes"),
                        Record::new(
                            record.timestamp,
                            record.process.memory.to_string(),
                            units::Unit::Bytes,
                        ),
                    ),
                );
                res.insert(
                    String::from("scaph_process_disk_write_bytes"),
                    (
                        String::from("Data written on disk by the process, in bytes"),
                        Record::new(
                            record.timestamp,
                            record.process.disk_written.to_string(),
                            units::Unit::Bytes,
                        ),
                    ),
                );
                res.insert(
                    String::from("scaph_process_disk_read_bytes"),
                    (
                        String::from("Data read on disk by the process, in bytes"),
                        Record::new(
                            record.timestamp,
                            record.process.disk_read.to_string(),
                            units::Unit::Bytes,
                        ),
                    ),
                );
                res.insert(
                    String::from("scaph_process_disk_total_write_bytes"),
                    (
                        String::from("Total data written on disk by the process, in bytes"),
                        Record::new(
                            record.timestamp,
                            record.process.total_disk_written.to_string(),
                            units::Unit::Bytes,
                        ),
                    ),
                );
                res.insert(
                    String::from("scaph_process_disk_total_read_bytes"),
                    (
                        String::from("Total data read on disk by the process, in bytes"),
                        Record::new(
                            record.timestamp,
                            record.process.total_disk_read.to_string(),
                            units::Unit::Bytes,
                        ),
                    ),
                );
            }
            if let Some(core_percentages) = core_percentages {
                let result = self.get_core_diff_power_microwatts().map(|r| {
                    r.values
                        .iter()
                        .enumerate()
                        .map(|t| {
                            let conso = t.1;
                            let percentage = core_percentages[t.0];
                            conso.parse::<f64>().unwrap_or(0_f64) * percentage
                        }).sum::<f64>()
                }).unwrap_or(0_f64);
                debug!("Simplified and correct process {pid} energy {result}");
                debug!("EBPF Core percentage {} for process {pid} Power {result}", core_percentages.iter().map(|p| p.to_string()).collect::<Vec<String>>().join(", "));
                res.insert(
                    String::from("scaph_process_power_consumption_microwatts"),
                    (
                        String::from("Total data read on disk by the process, in bytes"),
                        Record::new(record.timestamp, result.to_string(), units::Unit::MicroWatt),
                    ),
                );
            }
        }
        Some(res)
    }

    // Per process metrics, from ProcessRecord during last refresh, returned in Record structs

    pub fn get_process_cpu_usage_percentage(&self, pid: Pid) -> Option<Record> {
        if let Some(record) = self.get_proc_tracker().get_process_last_record(pid) {
            return Some(Record::new(
                record.timestamp,
                (record.process.cpu_usage_percentage / self.proc_tracker.nb_cores as f32)
                    .to_string(),
                units::Unit::Percentage,
            ));
        }
        None
    }

    pub fn get_process_memory_virtual_bytes(&self, pid: Pid) -> Option<Record> {
        if let Some(record) = self.get_proc_tracker().get_process_last_record(pid) {
            return Some(Record::new(
                record.timestamp,
                record.process.virtual_memory.to_string(),
                units::Unit::Bytes,
            ));
        }
        None
    }

    pub fn get_process_memory_bytes(&self, pid: Pid) -> Option<Record> {
        if let Some(record) = self.get_proc_tracker().get_process_last_record(pid) {
            return Some(Record::new(
                record.timestamp,
                record.process.memory.to_string(),
                units::Unit::Bytes,
            ));
        }
        None
    }

    pub fn get_process_disk_written_bytes(&self, pid: Pid) -> Option<Record> {
        if let Some(record) = self.get_proc_tracker().get_process_last_record(pid) {
            return Some(Record::new(
                record.timestamp,
                record.process.disk_written.to_string(),
                units::Unit::Bytes,
            ));
        }
        None
    }

    pub fn get_process_disk_read_bytes(&self, pid: Pid) -> Option<Record> {
        if let Some(record) = self.get_proc_tracker().get_process_last_record(pid) {
            return Some(Record::new(
                record.timestamp,
                record.process.disk_read.to_string(),
                units::Unit::Bytes,
            ));
        }
        None
    }
    pub fn get_process_disk_total_read_bytes(&self, pid: Pid) -> Option<Record> {
        if let Some(record) = self.get_proc_tracker().get_process_last_record(pid) {
            return Some(Record::new(
                record.timestamp,
                record.process.total_disk_read.to_string(),
                units::Unit::Bytes,
            ));
        }
        None
    }

    pub fn get_process_disk_total_write_bytes(&self, pid: Pid) -> Option<Record> {
        if let Some(record) = self.get_proc_tracker().get_process_last_record(pid) {
            return Some(Record::new(
                record.timestamp,
                record.process.total_disk_written.to_string(),
                units::Unit::Bytes,
            ));
        }
        None
    }

    #[cfg(target_os = "linux")]
    pub fn get_rapl_psys_energy_microjoules(&self) -> Option<Record> {
        if let Some(psys) = self._sensor_data.get("psys") {
            match &fs::read_to_string(format!("{psys}/energy_uj")) {
                Ok(val) => {
                    debug!("Read PSYS from {psys}/energy_uj: {}", val.to_string());
                    return Some(Record::new(
                        current_system_time_since_epoch(),
                        val.to_string(),
                        units::Unit::MicroJoule,
                    ));
                }
                Err(e) => {
                    warn!("PSYS Error: {:?}", e);
                }
            }
        } else {
            debug!("Asked for PSYS but there is no psys entry in sensor_data.");
        }
        None
    }

    /// # Safety
    ///
    /// This function is unsafe rust as it calls get_msr_value function from msr_rapl sensor module.
    /// It calls the msr_RAPL::MSR_PLATFORM_ENERGY_STATUS MSR address, which has been tested on several Intel x86 processors
    /// but might fail on AMD (needs testing). That being said, it returns None if the msr query fails (which means if the Windows
    /// driver fails.) and should not prevent from using a value coming from elsewhere, which means from another get_msr_value calls
    /// targeting another msr address.
    #[cfg(target_os = "windows")]
    pub unsafe fn get_rapl_psys_energy_microjoules(&self) -> Option<Record> {
        let msr_addr = msr_rapl::MSR_PLATFORM_ENERGY_STATUS;
        match get_msr_value(0, msr_addr.into(), &self._sensor_data) {
            Ok(res) => {
                return Some(Record::new(
                    current_system_time_since_epoch(),
                    res.value.to_string(),
                    units::Unit::MicroJoule,
                ))
            }
            Err(e) => {
                debug!("get_msr_value returned error : {}", e);
            }
        }
        None
    }
}


// !!!!!!!!!!!!!!!!! CPUSocket !!!!!!!!!!!!!!!!!!!!!!!
/// CPUSocket struct represents a CPU socket (matches physical_id attribute in /proc/cpuinfo),
/// owning CPU cores (processor in /proc/cpuinfo).
#[derive(Debug, Clone)]
pub struct CPUSocket {
    /// Numerical ID of the CPU socket (physical_id in /proc/cpuinfo)
    pub id: u16,
    /// RAPL domains attached to the socket
    pub domains: Vec<Domain>,
    /// Text attributes linked to that socket, found in /proc/cpuinfo
    pub attributes: Vec<Vec<HashMap<String, String>>>,
    /// Path to the file that provides the counter for energy consumed by the socket, in microjoules.
    pub counter_uj_path: String,
    /// Comsumption records measured and stored by scaphandre for this socket.
    pub record_buffer: Vec<Record>,
    /// Maximum size of the record_buffer in kilobytes.
    pub buffer_max_kbytes: u16,

    /// Idle comsumption records measured and stored by scaphandre for this socket.
    pub idle_record_buffer: Vec<Record>,

    /// CPU cores (core_id in /proc/cpuinfo) attached to the socket.
    pub cpu_cores: Vec<CPUCore>,
    /// Usage statistics records stored for this socket.
    pub stat_buffer: Vec<CPUStat>,

    /// Idle threshold for measurement of idle energy
    pub idle_percentage_threshold: f64,
    ///
    #[allow(dead_code)]
    pub sensor_data: HashMap<String, String>,
}

impl RecordGenerator for CPUSocket {
    /// Generates a new record of the socket energy consumption and stores it in the record_buffer.
    /// Returns a clone of this Record instance.
    fn refresh_record(&mut self) {
        match self.read_record() {
            Ok(record) => {
                self.record_buffer.push(record);
            }
            Err(e) => {
                warn!(
                    "Could'nt read record from {}, error was: {:?}",
                    self.sensor_data
                        .get("source_file")
                        .unwrap_or(&String::from("SRCFILENOTKNOWN")),
                    e
                );
            }
        }

        if let Some(record) = self.read_idle_record() {
            self.idle_record_buffer.push(record);
        }

        if !self.record_buffer.is_empty() {
            self.clean_old_records();
        }
    }

    /// Checks the size in memory of record_buffer and deletes as many Record
    /// instances from the buffer to make it smaller in memory than buffer_max_kbytes.
    fn clean_old_records(&mut self) {
        let record_ptr = &self.record_buffer[0];
        let curr_size = size_of_val(record_ptr) * self.record_buffer.len();
        trace!(
            "socket rebord buffer current size: {} max_bytes: {}",
            curr_size,
            self.buffer_max_kbytes * 1000
        );
        if curr_size > (self.buffer_max_kbytes * 1000) as usize {
            let size_diff = curr_size - (self.buffer_max_kbytes * 1000) as usize;
            trace!(
                "socket record size_diff: {} sizeof: {}",
                size_diff,
                size_of_val(record_ptr)
            );
            if size_diff > size_of_val(record_ptr) {
                let nb_records_to_delete = size_diff as f32 / size_of_val(record_ptr) as f32;
                for _ in 1..nb_records_to_delete as u32 {
                    if !self.record_buffer.is_empty() {
                        let res = self.record_buffer.remove(0);
                        debug!(
                            "Cleaning socket id {} records buffer, removing: {}",
                            self.id, res
                        );
                    }
                }
            }
        }

        if !self.idle_record_buffer.is_empty() {
            let idle_record_ptr = &self.idle_record_buffer[0];
            let idle_curr_size = size_of_val(idle_record_ptr) * self.idle_record_buffer.len();
            trace!(
                "socket idle record buffer current size: {} max_bytes: {}",
                idle_curr_size,
                self.buffer_max_kbytes * 1000
            );
            if idle_curr_size > (self.buffer_max_kbytes * 1000) as usize {
                let size_diff = idle_curr_size - (self.buffer_max_kbytes * 1000) as usize;
                trace!(
                    "socket idle record size_diff: {} sizeof: {}",
                    size_diff,
                    size_of_val(idle_record_ptr)
                );
                if size_diff > size_of_val(idle_record_ptr) {
                    let nb_records_to_delete =
                        size_diff as f32 / size_of_val(idle_record_ptr) as f32;
                    for _ in 1..nb_records_to_delete as u32 {
                        if !self.idle_record_buffer.is_empty() {
                            let res = self.idle_record_buffer.remove(0);
                            debug!(
                                "Cleaning socket id {} idle records buffer, removing: {}",
                                self.id, res
                            );
                        }
                    }
                }
            }
        }
    }

    /// Returns a new owned Vector being a clone of the current record_buffer.
    /// This does not affect the current buffer but is costly.
    fn get_records_passive(&self) -> Vec<Record> {
        let mut result = vec![];
        for r in &self.record_buffer {
            result.push(Record::new(
                r.timestamp,
                r.value.clone(),
                units::Unit::MicroJoule,
            ));
        }
        result
    }
}

impl CPUSocket {
    /// Creates and returns a CPUSocket instance with an empty buffer and no CPUCore owned yet.
    fn new(
        id: u16,
        domains: Vec<Domain>,
        attributes: Vec<Vec<HashMap<String, String>>>,
        counter_uj_path: String,
        buffer_max_kbytes: u16,
        sensor_data: HashMap<String, String>,
    ) -> CPUSocket {
        CPUSocket {
            id,
            domains,
            attributes,
            counter_uj_path,
            record_buffer: vec![], // buffer has to be empty first
            buffer_max_kbytes,
            idle_record_buffer: vec![],
            cpu_cores: vec![], // cores are instantiated on a later step
            stat_buffer: vec![],
            sensor_data,
            // idle_percentage_threshold: 0.95_f64,
            idle_percentage_threshold: 0.65_f64,
        }
    }

    pub fn set_id(&mut self, id: u16) {
        self.id = id
    }

    /// Adds a new Domain instance to the domains vector if and only if it doesn't exist in the vector already.
    fn safe_add_domain(&mut self, domain: Domain) {
        if !self.domains.iter().any(|d| d.id == domain.id) {
            self.domains.push(domain);
        }
    }

    /// Returns a mutable reference to the domains vector.
    pub fn get_domains(&mut self) -> &mut Vec<Domain> {
        &mut self.domains
    }

    /// Returns a immutable reference to the domains vector.
    pub fn get_domains_passive(&self) -> &Vec<Domain> {
        &self.domains
    }

    /// Returns a mutable reference to the CPU cores vector.
    pub fn get_cores(&mut self) -> &mut Vec<CPUCore> {
        &mut self.cpu_cores
    }

    /// Returns a immutable reference to the CPU cores vector.
    pub fn get_cores_passive(&self) -> &Vec<CPUCore> {
        &self.cpu_cores
    }

    /// Adds a CPU core instance to the cores vector.
    pub fn add_cpu_core(&mut self, core: CPUCore) {
        self.cpu_cores.push(core);
    }

    /// Reads a new record for socket-level idle energy
    pub fn read_idle_record(&self) -> Option<Record> {
        if let Some(stat) = self.get_stats_diff() {
            let (idle, total) = stat.get_idle_and_total();
            debug!("CPUSocket {} idle percentage {}", self.id, idle as f64 / total as f64);
            if (idle as f64 / total as f64) >= self.idle_percentage_threshold {
                if let Some(Ok(mut conso_core)) = self.get_records_diff_power_microwatts().map(|r| r.value.parse::<u64>()) {
                    if let Some(Ok(idle_conso)) = self.get_idle_power_microwatts().map(|r| r.value.parse::<u64>()) {
                        conso_core = if conso_core > 0_u64 {
                            let min_conso = min(conso_core, idle_conso);
                            debug!("Found Lower IDLE Consumption: {min_conso}");
                            min_conso
                        } else {
                            idle_conso
                        };
                    }
                    return Some(Record::new(
                        current_system_time_since_epoch(),
                        conso_core.to_string(),
                        units::Unit::MicroWatt,
                    ));
                }
            }
        }
        None
    }

    /// Returns current idle energy
    pub fn get_idle_power_microwatts(&self) -> Option<Record> {
        debug!("Inside idle power calculation function");
        if !self.idle_record_buffer.is_empty() {
            let last = self.idle_record_buffer.iter().last();

            if let Some(last) = last {
                debug!("IDLE Consumption {}", last.value);
                return Some(Record::new(
                    current_system_time_since_epoch(),
                    // read_idle_record either keeps the current power or goes lower
                    last.value.clone(),
                    units::Unit::MicroWatt,
                ));
            }
        }
        None
    }


    /// Generates a new CPUStat object storing current usage statistics of the socket
    /// and stores it in the stat_buffer.
    pub fn refresh_stats(&mut self) {
        if !self.stat_buffer.is_empty() {
            self.clean_old_stats();
        }
        self.stat_buffer.insert(0, self.read_stats().unwrap());
    }

    /// Checks the size in memory of stats_buffer and deletes as many CPUStat
    /// instances from the buffer to make it smaller in memory than buffer_max_kbytes.
    fn clean_old_stats(&mut self) {
        let stat_ptr = &self.stat_buffer[0];
        let size_of_stat = size_of_val(stat_ptr);
        let curr_size = size_of_stat * self.stat_buffer.len();
        trace!("current_size of stats in socket {}: {}", self.id, curr_size);
        trace!(
            "estimated max nb of socket stats: {}",
            self.buffer_max_kbytes as f32 * 1000.0 / size_of_stat as f32
        );
        if curr_size > (self.buffer_max_kbytes * 1000) as usize {
            let size_diff = curr_size - (self.buffer_max_kbytes * 1000) as usize;
            trace!(
                "socket {} size_diff: {} size of: {}",
                self.id,
                size_diff,
                size_of_stat
            );
            if size_diff > size_of_stat {
                let nb_stats_to_delete = size_diff as f32 / size_of_stat as f32;
                trace!(
                    "socket {} nb_stats_to_delete: {} size_diff: {} size of: {}",
                    self.id,
                    nb_stats_to_delete,
                    size_diff,
                    size_of_stat
                );
                trace!("nb stats to delete: {}", nb_stats_to_delete as u32);
                for _ in 1..nb_stats_to_delete as u32 {
                    if !self.stat_buffer.is_empty() {
                        let res = self.stat_buffer.pop();
                        debug!(
                            "Cleaning stat buffer of socket {}, removing: {:?}",
                            self.id, res
                        );
                    }
                }
            }
        }
    }

    /// Combines stats from all CPU cores owned byu the socket and returns
    /// a CpuStat struct containing stats for the whole socket.
    pub fn read_stats(&self) -> Option<CPUStat> {
        let mut stats = CPUStat {
            user: 0,
            nice: 0,
            system: 0,
            idle: 0,
            iowait: Some(0),
            irq: Some(0),
            softirq: Some(0),
            guest: Some(0),
            guest_nice: Some(0),
            steal: Some(0),
        };
        for c in &self.cpu_cores {
            if let Some(c_stats) = c.read_stats() {
                stats.user += c_stats.user;
                stats.nice += c_stats.nice;
                stats.system += c_stats.system;
                stats.idle += c_stats.idle;
                stats.iowait =
                    Some(stats.iowait.unwrap_or_default() + c_stats.iowait.unwrap_or_default());
                stats.irq = Some(stats.irq.unwrap_or_default() + c_stats.irq.unwrap_or_default());
                stats.softirq =
                    Some(stats.softirq.unwrap_or_default() + c_stats.softirq.unwrap_or_default());
            }
        }
        Some(stats)
    }

    /// Computes the difference between previous usage statistics record for the socket
    /// and the current one. Returns a CPUStat object containing this difference, field
    /// by field.
    pub fn get_stats_diff(&self) -> Option<CPUStat> {
        if self.stat_buffer.len() > 1 {
            let last = &self.stat_buffer[0];
            let previous = &self.stat_buffer[1];
            let mut iowait = None;
            let mut irq = None;
            let mut softirq = None;
            let mut steal = None;
            let mut guest = None;
            let mut guest_nice = None;
            if last.iowait.is_some() && previous.iowait.is_some() {
                iowait = Some(last.iowait.unwrap() - previous.iowait.unwrap());
            }
            if last.irq.is_some() && previous.irq.is_some() {
                irq = Some(last.irq.unwrap() - previous.irq.unwrap());
            }
            if last.softirq.is_some() && previous.softirq.is_some() {
                softirq = Some(last.softirq.unwrap() - previous.softirq.unwrap());
            }
            if last.steal.is_some() && previous.steal.is_some() {
                steal = Some(last.steal.unwrap() - previous.steal.unwrap());
            }
            if last.guest.is_some() && previous.guest.is_some() {
                guest = Some(last.guest.unwrap() - previous.guest.unwrap());
            }
            if last.guest_nice.is_some() && previous.guest_nice.is_some() {
                guest_nice = Some(last.guest_nice.unwrap() - previous.guest_nice.unwrap());
            }
            return Some(CPUStat {
                user: last.user - previous.user,
                nice: last.nice - previous.nice,
                system: last.system - previous.system,
                idle: last.idle - previous.idle,
                iowait,
                irq,
                softirq,
                steal,
                guest,
                guest_nice,
            });
        }
        None
    }

    /// Returns a Record instance containing the power consumed between last
    /// and previous measurement, for this CPU socket
    pub fn get_records_diff_power_microwatts(&self) -> Option<Record> {
        if self.record_buffer.len() > 1 {
            let last_record = self.record_buffer.last().unwrap();
            let previous_record = self
                .record_buffer
                .get(self.record_buffer.len() - 2)
                .unwrap();
            debug!(
                "socket : last_record value: {} previous_record value: {}",
                &last_record.value, &previous_record.value
            );
            let last_rec_val = last_record.value.trim();
            debug!("socket : l1187 : trying to parse {} as u64", last_rec_val);
            let prev_rec_val = previous_record.value.trim();
            debug!("socket : l118 : trying to parse {} as u64", prev_rec_val);
            if let (Ok(last_microjoules), Ok(previous_microjoules)) =
                (last_rec_val.parse::<u64>(), prev_rec_val.parse::<u64>())
            {
                let mut microjoules = 0;
                if last_microjoules >= previous_microjoules {
                    microjoules = last_microjoules - previous_microjoules;
                } else {
                    debug!(
                        "socket: previous_microjoules ({}) > last_microjoules ({})",
                        previous_microjoules, last_microjoules
                    );
                }
                let time_diff =
                    last_record.timestamp.as_secs_f64() - previous_record.timestamp.as_secs_f64();
                let microwatts = microjoules as f64 / time_diff;
                debug!("socket : l1067: microwatts: {}", microwatts);
                debug!("Calculating Socket with IDLE.");
                return Some(Record::new(
                    last_record.timestamp,
                    (
                        (microwatts as u64)
                            .saturating_sub(self
                                .get_idle_power_microwatts()
                                .map(|r| r.value.parse::<u64>().unwrap_or(0))
                                .unwrap_or(0)
                            )).to_string(),
                    units::Unit::MicroWatt,
                ));
            }
        } else {
            warn!("Not enough records for socket");
        }
        None
    }

    pub fn get_rapl_mmio_energy_microjoules(&self) -> Option<Record> {
        if let Some(mmio) = self.sensor_data.get("mmio") {
            match &fs::read_to_string(mmio) {
                Ok(val) => {
                    return Some(Record::new(
                        current_system_time_since_epoch(),
                        val.to_string(),
                        units::Unit::MicroJoule,
                    ));
                }
                Err(e) => {
                    debug!("MMIO Error: {:?}", e)
                }
            }
        }
        None
    }
}

// !!!!!!!!!!!!!!!!! CPUCore !!!!!!!!!!!!!!!!!!!!!!!
/// CPUCore reprensents each CPU core on the host,
/// owned by a CPUSocket. CPUCores are instanciated regardless if
/// HyperThreading is activated on the host.
/// Reprensents the processor field in /proc/cpuinfo.
#[derive(Debug)]
pub struct CPUCore {
    pub id: u16,
    pub attributes: HashMap<String, String>,
    /// Idle CPU time records (sum of all non-C0 cpuidle states, in microseconds)
    pub record_buffer: Vec<MultiValuedRecord>,
    /// Maximum size of record_buffer in kilobytes
    pub buffer_max_kbytes: u16,
    /// Counter for core instructions
    instructions: Option<RefCell<Counter>>,
    /// Counter for core cycles
    cycles: Option<RefCell<Counter>>,
}

impl Clone for CPUCore {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            attributes: self.attributes.clone(),
            record_buffer: self.record_buffer.clone(),
            buffer_max_kbytes: self.buffer_max_kbytes,
            instructions: None,
            cycles: None,
        }
    }
}

impl MultiValuedRecordGenerator for CPUCore {
    /// Refresh and store a new cpuidle idle time record for this core.
    fn refresh_record(&mut self) {
        match self.read_record() {
            Ok(record) => {
                self.record_buffer.push(record);
            }
            Err(e) => {
                debug!(
                    "Couldn't read cpuidle record from cpu{}: {:?}",
                    self.id, e
                );
            }
        }
        if !self.record_buffer.is_empty() {
            self.clean_old_records();
        }
    }

    /// Remove old records to keep buffer size within limit.
    fn clean_old_records(&mut self) {
        if self.record_buffer.is_empty() {
            return;
        }
        let record_ptr = &self.record_buffer[0];
        let curr_size = size_of_val(record_ptr) * self.record_buffer.len();
        if curr_size > (self.buffer_max_kbytes * 1000) as usize {
            let size_diff = curr_size - (self.buffer_max_kbytes * 1000) as usize;
            if size_diff > size_of_val(record_ptr) {
                let nb_records_to_delete = size_diff as f32 / size_of_val(record_ptr) as f32;
                for _ in 1..nb_records_to_delete as u32 {
                    if !self.record_buffer.is_empty() {
                        self.record_buffer.remove(0);
                    }
                }
            }
        }
    }

    /// Return a copy of all records in the buffer.
    fn get_records_passive(&self) -> Vec<MultiValuedRecord> {
        let mut result = vec![];
        for r in &self.record_buffer {
            result.push(MultiValuedRecord::new(
                r.timestamp,
                r.values.to_vec(),
                r.units.clone(),
            ));
        }
        result
    }
}

impl MultiValuedRecordReader for CPUCore {
    /// Read idle CPU time (non-C0 cpuidle states sum) from sysfs for this core.
    /// Returns the total idle time in microseconds as a Record.
    fn read_record(&self) -> Result<MultiValuedRecord, Box<dyn Error>> {
        #[cfg(target_os = "linux")]
        {
            let mut total_idle_us: u64 = 0;
            let base_path = format!("/sys/devices/system/cpu/cpu{}/cpuidle", self.id);

            if let Ok(entries) = fs::read_dir(&base_path) {
                for entry in entries.flatten() {
                    let state_path = entry.path();
                    if state_path.is_dir() {
                        let state_name = state_path
                            .file_name()
                            .and_then(|n| n.to_str());

                        if let Some(name) = state_name {
                            // Skip state0 (C0, active state)
                            if name == "state0" {
                                continue;
                            }
                            let time_file = state_path.join("time");
                            if let Ok(time_str) = fs::read_to_string(time_file) {
                                if let Ok(time_us) = time_str.trim().parse::<u64>() {
                                    total_idle_us += time_us;
                                }
                            }
                        }
                    }
                }
            }

            // Read APERF and MPERF from /dev/cpu/<id>/msr
            const MSR_IA32_MPERF: u64 = 0xE7;
            const MSR_IA32_APERF: u64 = 0xE8;

            let read_msr = |msr: u64| -> Option<u64> {
                let file = fs::File::open(format!("/dev/cpu/{}/msr", self.id)).ok()?;
                let mut buf = [0u8; 8];
                file.read_at(&mut buf, msr).ok()?;
                Some(u64::from_le_bytes(buf))
            };

            let mperf = read_msr(MSR_IA32_MPERF).unwrap_or(0);
            let aperf = read_msr(MSR_IA32_APERF).unwrap_or(0);

            // Logic to retrieve core cpu time
            let (core_busy, core_total, node_busy, node_total) =
                KernelStats::new()
                    .ok()
                    .and_then(|mut ks| {
                        let nt = &ks.total;
                        let n_total = nt.user + nt.nice + nt.system + nt.idle
                            + nt.iowait.unwrap_or(0) + nt.irq.unwrap_or(0)
                            + nt.softirq.unwrap_or(0) + nt.steal.unwrap_or(0)
                            + nt.guest.unwrap_or(0) + nt.guest_nice.unwrap_or(0);
                        let n_busy = n_total - nt.idle - nt.iowait.unwrap_or(0);
                        ks.cpu_time.get(self.id as usize).map(|ct| {
                            let total = ct.user + ct.nice + ct.system + ct.idle
                                + ct.iowait.unwrap_or(0) + ct.irq.unwrap_or(0)
                                + ct.softirq.unwrap_or(0) + ct.steal.unwrap_or(0)
                                + ct.guest.unwrap_or(0) + ct.guest_nice.unwrap_or(0);
                            let busy = total - ct.idle - ct.iowait.unwrap_or(0);
                            (busy, total, n_busy, n_total)
                        })
                    })
                    .unwrap_or((0, 0, 0, 0));

            let instr = self.instructions.as_ref().and_then(|c| c.borrow_mut().read_count_and_time().ok());
            let cyc = self.cycles.as_ref().and_then(|c| c.borrow_mut().read_count_and_time().ok());

            // Correct for multiplexing: if time_running < time_enabled, the kernel
            // only scheduled the counter onto hardware for part of the interval.
            let instr_scaled = instr.map(|instr| if instr.time_running > 0 {
                instr.count as f64 * (instr.time_enabled as f64 / instr.time_running as f64)
            } else { 0.0 }).unwrap_or(0.0);
            let cyc_scaled = cyc.map(|cyc| if cyc.time_running > 0 {
                cyc.count as f64 * (cyc.time_enabled as f64 / cyc.time_running as f64)
            } else { 0.0 }).unwrap_or(0.0);

            Ok(MultiValuedRecord::new(
                current_system_time_since_epoch(),
                vec![
                    total_idle_us.to_string(),
                    mperf.to_string(),
                    aperf.to_string(),
                    core_busy.to_string(),
                    core_total.to_string(),
                    node_busy.to_string(),
                    node_total.to_string(),
                    instr_scaled.to_string(),
                    cyc_scaled.to_string(),
                ],
                vec![
                    units::Unit::MicroSeconds,
                    units::Unit::Cycles,
                    units::Unit::Cycles,
                    units::Unit::Numeric,
                    units::Unit::Numeric,
                    units::Unit::Numeric,
                    units::Unit::Numeric,
                    units::Unit::Numeric,
                    units::Unit::Numeric,
                ],
            ))
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err("cpuidle reading only supported on Linux".into())
        }
    }
}

impl CPUCore {
    /// Instantiates CPUCore and returns the instance.
    pub fn new(id: u16, attributes: HashMap<String, String>) -> CPUCore {

        let cpu_id = id as usize;

        let mut instructions = Builder::new(Hardware::INSTRUCTIONS)
            .one_cpu(cpu_id)
            .any_pid()
            .build().ok();
        let mut cycles = Builder::new(Hardware::CPU_CYCLES)
            .one_cpu(cpu_id)
            .any_pid()
            .build().ok();

        if let Some(ref mut i) = instructions {
            let _ = i.enable();
        }
        if let Some(ref mut c) = cycles {
            let _ = c.enable();
        }

        CPUCore {
            id,
            attributes,
            record_buffer: vec![],
            buffer_max_kbytes: 1,
            instructions: instructions.map(|inst| RefCell::new(inst)),
            cycles: cycles.map(|cyc| RefCell::new(cyc)),
        }
    }

    /// Reads content from /proc/stat and extracts the stats of the CPU core
    fn read_stats(&self) -> Option<CPUStat> {
        #[cfg(target_os = "linux")]
        {
            if let Ok(mut kernelstats) = KernelStats::new() {
                return Some(CPUStat::from_procfs_cputime(
                    kernelstats.cpu_time.remove(self.id as usize),
                ));
            }
        }
        None
    }

    /// Returns the difference in idle CPU time (microseconds) between the last two records.
    /// Returns None if there are fewer than 2 records.
    pub fn get_core_metrics_delta(&self) -> Option<CPUCoreMetrics> {
        if self.record_buffer.len() > 1 {
            let last = self.record_buffer.last().unwrap();
            let previous = self.record_buffer.get(self.record_buffer.len() - 2).unwrap();
            let mut res = CPUCoreMetrics{
                average_frequency: 0,
                active_percentage: 0_f64,
                cpu_time_percentage: 0.0,
                active_time: 0,
                aperf: 0,
                mperf: 0,
                inst: 0.0,
                cyc: 0.0,
                ipc: 0.0,
            };

            if last.values.len() >= 9 && previous.values.len() >= 9 {
                let core_busy_delta = last.values[3].trim().parse::<u64>().unwrap_or(0)
                    .saturating_sub(previous.values[3].trim().parse::<u64>().unwrap_or(0));
                let core_total_delta = last.values[4].trim().parse::<u64>().unwrap_or(0)
                    .saturating_sub(previous.values[4].trim().parse::<u64>().unwrap_or(0));

                res.active_percentage = if core_total_delta != 0 {
                    core_busy_delta as f64 / core_total_delta as f64
                } else {
                    0_f64
                };

                let aperf_delta = last.values[2].trim().parse::<u64>().unwrap_or(0)
                    .saturating_sub(previous.values[2].trim().parse::<u64>().unwrap_or(0));
                let mperf_delta = last.values[1].trim().parse::<u64>().unwrap_or(0)
                    .saturating_sub(previous.values[1].trim().parse::<u64>().unwrap_or(0));

                res.aperf = aperf_delta;
                res.mperf = mperf_delta;

                let max_freq_khz = fs::read_to_string(format!(
                    "/sys/devices/system/cpu/cpu{}/cpufreq/cpuinfo_max_freq",
                    self.id
                ))
                .ok()
                .and_then(|s| s.trim().parse::<u64>().ok())
                .unwrap_or(0);

                res.average_frequency = if mperf_delta != 0 {
                    (max_freq_khz as f64 * aperf_delta as f64 / mperf_delta as f64) as u64
                } else {
                    0
                };

                let core_busy_delta = last.values[3].trim().parse::<u64>().unwrap_or(0)
                    .saturating_sub(previous.values[3].trim().parse::<u64>().unwrap_or(0));
                res.active_time = core_busy_delta;
                let node_busy_delta = last.values[5].trim().parse::<u64>().unwrap_or(0)
                    .saturating_sub(previous.values[5].trim().parse::<u64>().unwrap_or(0));
                res.cpu_time_percentage = if node_busy_delta != 0 {
                    core_busy_delta as f64 / node_busy_delta as f64 * 100.0
                } else {
                    0.0
                };


                res.inst = last.values[7].trim().parse::<f64>().unwrap_or(0.0) -
                    previous.values[7].trim().parse::<f64>().unwrap_or(0.0);
                res.cyc = last.values[8].trim().parse::<f64>().unwrap_or(0.0) - 
                    previous.values[8].trim().parse::<f64>().unwrap_or(0.0);

                res.ipc = if res.cyc > 0.0 { res.inst / res.cyc } else { 0.0 };

                return Some(res);
            }
        }
        None
    }
}

// !!!!!!!!!!!!!!!!! Domain !!!!!!!!!!!!!!!!!!!!!!!
/// Domain struct represents a part of a CPUSocket from the
/// electricity consumption point of view.
#[derive(Debug, Clone)]
pub struct Domain {
    /// Numerical ID of the RAPL domain as indicated in /sys/class/powercap/intel-rapl* folders names
    pub id: u16,
    /// Name of the domain as found in /sys/class/powercap/intel-rapl:X:X/name
    pub name: String,
    /// Path to the domain's energy counter file, microjoules extracted
    pub counter_uj_path: String,
    /// History of energy consumption measurements, stored as Record instances
    pub record_buffer: Vec<Record>,
    /// Maximum size of record_buffer, in kilobytes
    pub buffer_max_kbytes: u16,
    ///
    #[allow(dead_code)]
    sensor_data: HashMap<String, String>,
}
impl RecordGenerator for Domain {
    /// Computes a measurement of energy comsumption for this CPU domain,
    /// stores a copy in self.record_buffer and returns it.
    fn refresh_record(&mut self) {
        match self.read_record() {
            Ok(record) => {
                self.record_buffer.push(record);
            }
            Err(e) => {
                warn!(
                    "Could'nt read record from {}. Error was : {:?}.",
                    self.sensor_data
                        .get("source_file")
                        .unwrap_or(&String::from("SRCFILENOTKNOWN")),
                    e
                );
            }
        }

        if !self.record_buffer.is_empty() {
            self.clean_old_records();
        }
    }

    /// Removes as many Record instances from self.record_buffer as needed
    /// for record_buffer to take less than 'buffer_max_kbytes' in memory
    fn clean_old_records(&mut self) {
        let record_ptr = &self.record_buffer[0];
        let curr_size = size_of_val(record_ptr) * self.record_buffer.len();
        if curr_size > (self.buffer_max_kbytes * 1000) as usize {
            let size_diff = curr_size - (self.buffer_max_kbytes * 1000) as usize;
            if size_diff > size_of_val(&self.record_buffer[0]) {
                let nb_records_to_delete =
                    size_diff as f32 / size_of_val(&self.record_buffer[0]) as f32;
                for _ in 1..nb_records_to_delete as u32 {
                    if !self.record_buffer.is_empty() {
                        self.record_buffer.remove(0);
                    }
                }
            }
        }
    }

    /// Returns a copy of self.record_buffer
    fn get_records_passive(&self) -> Vec<Record> {
        let mut result = vec![];
        for r in &self.record_buffer {
            result.push(Record::new(
                r.timestamp,
                r.value.clone(),
                units::Unit::MicroJoule,
            ));
        }
        result
    }
}
impl Domain {
    /// Instanciates Domain and returns the instance
    fn new(
        id: u16,
        name: String,
        counter_uj_path: String,
        buffer_max_kbytes: u16,
        sensor_data: HashMap<String, String>,
    ) -> Domain {
        Domain {
            id,
            name,
            counter_uj_path,
            record_buffer: vec![],
            buffer_max_kbytes,
            sensor_data,
        }
    }

    /// Returns a Record instance containing the power consumed between
    /// last and previous measurement, in microwatts.
    pub fn get_records_diff_power_microwatts(&self) -> Option<Record> {
        if self.record_buffer.len() > 1 {
            let last_record = self.record_buffer.last().unwrap();
            let previous_record = self
                .record_buffer
                .get(self.record_buffer.len() - 2)
                .unwrap();
            if let (Ok(last_microjoules), Ok(previous_microjoules)) = (
                last_record.value.trim().parse::<u64>(),
                previous_record.value.trim().parse::<u64>(),
            ) {
                if previous_microjoules > last_microjoules {
                    return None;
                }
                let microjoules = last_microjoules - previous_microjoules;
                let time_diff =
                    last_record.timestamp.as_secs_f64() - previous_record.timestamp.as_secs_f64();
                let microwatts = microjoules as f64 / time_diff;
                return Some(Record::new(
                    last_record.timestamp,
                    (microwatts as u64).to_string(),
                    units::Unit::MicroWatt,
                ));
            }
        }
        None
    }

    pub fn get_rapl_mmio_energy_microjoules(&self) -> Option<Record> {
        if let Some(mmio) = self.sensor_data.get("mmio") {
            match &fs::read_to_string(mmio) {
                Ok(val) => {
                    return Some(Record::new(
                        current_system_time_since_epoch(),
                        val.to_string(),
                        units::Unit::MicroJoule,
                    ));
                }
                Err(e) => {
                    debug!("MMIO Error in get microjoules: {:?}", e);
                }
            }
        }
        None
    }
}
impl fmt::Display for Domain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Domain: {}", self.name)
    }
}

// !!!!!!!!!!!!!!!!! Record !!!!!!!!!!!!!!!!!!!!!!!
/// Record struct represents an electricity consumption measurement
/// tied to a domain.
#[derive(Debug, Clone)]
pub struct Record {
    pub timestamp: Duration,
    pub value: String,
    pub unit: units::Unit,
}

#[derive(Debug, Clone)]
pub struct MultiValuedRecord {
    pub timestamp: Duration,
    pub values: Vec<String>,
    pub units: Vec<units::Unit>,
}

#[derive(Debug, Clone)]
pub struct CPUCoreMetrics {
    average_frequency: u64,
    active_percentage: f64,
    cpu_time_percentage: f64,
    active_time: u64,
    aperf: u64,
    mperf: u64,
    inst: f64,
    cyc: f64,
    ipc: f64,
}

impl Record {
    /// Instances Record and returns the instance
    pub fn new(timestamp: Duration, value: String, unit: units::Unit) -> Record {
        Record {
            timestamp,
            value,
            unit,
        }
    }
}


impl MultiValuedRecord {
    /// Instances Record and returns the instance
    pub fn new(timestamp: Duration, values: Vec<String>, units: Vec<units::Unit>) -> MultiValuedRecord {
        MultiValuedRecord {
            timestamp,
            values,
            units,
        }
    }
}

impl fmt::Display for Record {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "recorded {} {} at {:?}",
            self.value.trim(),
            self.unit,
            self.timestamp
        )
    }
}


impl fmt::Display for MultiValuedRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "recorded [{}] [{}] at {:?}",
            self.values.join(", "),
            self.units.iter().map(|u| u.to_string()).collect::<Vec<String>>().join(", "),
            self.timestamp
        )
    }
}

#[derive(Debug)]
pub struct CPUStat {
    user: u64,
    nice: u64,
    system: u64,
    idle: u64,
    irq: Option<u64>,
    iowait: Option<u64>,
    softirq: Option<u64>,
    steal: Option<u64>,
    guest: Option<u64>,
    guest_nice: Option<u64>,
}

impl CPUStat {
    #[cfg(target_os = "linux")]
    pub fn from_procfs_cputime(cpu_time: CpuTime) -> CPUStat {
        CPUStat {
            user: cpu_time.user,
            nice: cpu_time.nice,
            system: cpu_time.system,
            idle: cpu_time.idle,
            irq: cpu_time.irq,
            iowait: cpu_time.iowait,
            softirq: cpu_time.softirq,
            steal: cpu_time.steal,
            guest: cpu_time.guest,
            guest_nice: cpu_time.guest_nice,
        }
    }

    /// Returns the total of active CPU time spent, for this stat measurement
    /// (not iowait, idle, irq or softirq)
    pub fn total_time_jiffies(&self) -> u64 {
        let user = self.user;
        let nice = self.nice;
        let system = self.system;
        let idle = self.idle;
        let irq = self.irq.unwrap_or_default();
        let iowait = self.iowait.unwrap_or_default();
        let softirq = self.softirq.unwrap_or_default();
        let steal = self.steal.unwrap_or_default();
        let guest_nice = self.guest_nice.unwrap_or_default();
        let guest = self.guest.unwrap_or_default();

        trace!(
            "CPUStat contains user {} nice {} system {} idle: {} irq {} softirq {} iowait {} steal {} guest_nice {} guest {}",
            user, nice, system, idle, irq, softirq, iowait, steal, guest_nice, guest
        );
        user + nice + system + guest_nice + guest
    }

    fn get_idle_and_total(&self) -> (u64, u64) {
        let idle = self.idle + self.iowait.unwrap_or(0);

        let total = self.user
            + self.nice
            + self.system
            + self.idle
            + self.iowait.unwrap_or(0)
            + self.irq.unwrap_or(0)
            + self.softirq.unwrap_or(0)
            + self.steal.unwrap_or(0)
            + self.guest.unwrap_or(0)
            + self.guest_nice.unwrap_or(0);

        (idle, total)
    }
}

impl Clone for CPUStat {
    /// Returns a copy of CPUStat instance
    fn clone(&self) -> CPUStat {
        CPUStat {
            user: self.user,
            guest: self.guest,
            guest_nice: self.guest_nice,
            idle: self.idle,
            iowait: self.iowait,
            irq: self.irq,
            nice: self.nice,
            softirq: self.softirq,
            steal: self.steal,
            system: self.system,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn get_proc_cpuinfo() {
        let cores = Topology::generate_cpu_cores().unwrap();
        println!(
            "cores: {} attributes in core 0: {}",
            cores.len(),
            cores[0].attributes.len()
        );
        for c in &cores {
            println!("{:?}", c.attributes);
        }
        assert_eq!(!cores.is_empty(), true);
        for c in &cores {
            assert_eq!(c.attributes.len() > 3, true);
        }
    }

    #[test]
    fn read_topology_stats() {
        #[cfg(target_os = "linux")]
        let sensor = powercap_rapl::PowercapRAPLSensor::new(8, 8, false);
        #[cfg(not(target_os = "linux"))]
        let sensor = msr_rapl::MsrRAPLSensor::new();
        let topo = (*sensor.get_topology()).unwrap();
        println!("{:?}", topo.read_stats());
    }

    #[test]
    fn read_core_stats() {
        #[cfg(target_os = "linux")]
        let sensor = powercap_rapl::PowercapRAPLSensor::new(8, 8, false);
        #[cfg(not(target_os = "linux"))]
        let sensor = msr_rapl::MsrRAPLSensor::new();
        let mut topo = (*sensor.get_topology()).unwrap();
        for s in topo.get_sockets() {
            for c in s.get_cores() {
                println!("{:?}", c.read_stats());
            }
        }
    }

    #[test]
    fn read_socket_stats() {
        #[cfg(target_os = "linux")]
        let sensor = powercap_rapl::PowercapRAPLSensor::new(8, 8, false);
        #[cfg(not(target_os = "linux"))]
        let sensor = msr_rapl::MsrRAPLSensor::new();
        let mut topo = (*sensor.get_topology()).unwrap();
        for s in topo.get_sockets() {
            println!("{:?}", s.read_stats());
        }
    }
}

//  Copyright 2020 The scaphandre authors.
//
//  Licensed under the Apache License, Version 2.0 (the "License");
//  you may not use this file except in compliance with the License.
//  You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
//  Unless required by applicable law or agreed to in writing, software
//  distributed under the License is distributed on an "AS IS" BASIS,
//  WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
//  See the License for the specific language governing permissions and
//  limitations under the License.
