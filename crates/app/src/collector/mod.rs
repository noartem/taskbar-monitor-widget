//! Background data collection. One thread samples system metrics at the refresh interval, another polls
//! Claude usage; both hand results to the UI thread through [`Shared`] and a posted window message.

pub mod claude;
pub mod gpu;
pub mod network;
pub mod pdh;
pub mod system;
pub mod wmi;

use std::collections::HashMap;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use tm_core::claude::ClaudeUsage;
use tm_core::config::{Claude, Hardware};
use tm_core::metrics::DataSources;
use tm_core::snapshot::{GpuInfo, SensorInfo, Snapshot};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::app::WM_APP_SNAPSHOT;

#[derive(Debug, Clone, PartialEq)]
pub struct Requirements {
    pub sources: DataSources,
    pub interval_ms: u32,
    pub hardware: Hardware,
    pub claude: Claude,
}

pub struct SharedState {
    pub req: Requirements,
    pub snapshot: Option<Snapshot>,
    pub claude: ClaudeUsage,
    pub claude_refresh: bool,
    /// Bumped when requirements change (wakes the sampler).
    pub generation: u64,
    /// Bumped when Claude settings change (wakes the poller).
    pub claude_generation: u64,
    pub stop: bool,
}

pub struct Shared {
    pub state: Mutex<SharedState>,
    pub wake: Condvar,
    pub claude_wake: Condvar,
    hwnd: AtomicIsize,
}

impl Shared {
    pub fn notify_ui(&self) {
        let hwnd = self.hwnd.load(Ordering::Relaxed);
        if hwnd != 0 {
            unsafe {
                let _ = PostMessageW(
                    Some(crate::util::hwnd_from_isize(hwnd)),
                    WM_APP_SNAPSHOT,
                    Default::default(),
                    Default::default(),
                );
            }
        }
    }
}

pub struct Collector {
    pub shared: Arc<Shared>,
    threads: Vec<std::thread::JoinHandle<()>>,
}

impl Collector {
    pub fn start(req: Requirements, hwnd: isize) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(SharedState {
                req,
                snapshot: None,
                claude: ClaudeUsage::default(),
                claude_refresh: false,
                generation: 0,
                claude_generation: 0,
                stop: false,
            }),
            wake: Condvar::new(),
            claude_wake: Condvar::new(),
            hwnd: AtomicIsize::new(hwnd),
        });
        let s1 = shared.clone();
        let s2 = shared.clone();
        let threads = vec![
            std::thread::Builder::new().name("sampler".into()).spawn(move || sampler(s1)).expect("sampler thread"),
            std::thread::Builder::new().name("claude".into()).spawn(move || claude::run(s2)).expect("claude thread"),
        ];
        Self { shared, threads }
    }

    pub fn update(&self, req: Requirements) {
        let mut st = self.shared.state.lock().unwrap();
        if st.req == req {
            return;
        }
        let claude_changed = st.req.claude != req.claude
            || st.req.sources.contains(DataSources::CLAUDE) != req.sources.contains(DataSources::CLAUDE);
        st.req = req;
        st.generation += 1;
        if claude_changed {
            st.claude_generation += 1;
        }
        drop(st);
        self.shared.wake.notify_all();
        self.shared.claude_wake.notify_all();
    }

    pub fn refresh_claude(&self) {
        self.shared.state.lock().unwrap().claude_refresh = true;
        self.shared.claude_wake.notify_all();
    }

    pub fn take_snapshot(&self) -> Option<Snapshot> {
        self.shared.state.lock().unwrap().snapshot.take()
    }

    pub fn stop(&mut self) {
        self.shared.state.lock().unwrap().stop = true;
        self.shared.wake.notify_all();
        self.shared.claude_wake.notify_all();
        // The Claude thread may be inside a network request (bounded by WinHTTP timeouts); do not block exit on it.
        if let Some(sampler) = self.threads.drain(..).next() {
            let _ = sampler.join();
        }
    }
}

/// Slow-changing data refreshed less often than the main interval.
struct Cache {
    cpu_name: Option<String>,
    adapters: Vec<gpu::Adapter>,
    adapters_at: Option<Instant>,
    drives: Vec<tm_core::snapshot::DriveInfo>,
    drives_at: Option<Instant>,
    sensors_at: Option<Instant>,
    sensors: Vec<SensorInfo>,
    cpu_temp: Option<f64>,
    gpu_sensors: HashMap<String, gpu::GpuSensor>,
}

fn stale(at: Option<Instant>, every: Duration) -> bool {
    at.is_none_or(|t| t.elapsed() >= every)
}

fn sampler(shared: Arc<Shared>) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    wmi::init_security();
    let elevated = crate::elevation::is_elevated();

    let mut pdh: Option<pdh::Pdh> = None;
    let mut net = network::NetMonitor::default();
    let mut cpu_times = system::CpuTimes::default();
    let mut gpu_sensors = gpu::GpuSensors::default();
    let mut wmi_client: Option<wmi::Wmi> = None;
    let mut cache = Cache {
        cpu_name: system::cpu_name(),
        adapters: Vec::new(),
        adapters_at: None,
        drives: Vec::new(),
        drives_at: None,
        sensors_at: None,
        sensors: Vec::new(),
        cpu_temp: None,
        gpu_sensors: HashMap::new(),
    };

    loop {
        let started = Instant::now();
        let (req, generation) = {
            let st = shared.state.lock().unwrap();
            if st.stop {
                return;
            }
            (st.req.clone(), st.generation)
        };
        let src = req.sources;
        let hw = &req.hardware;

        let needs = pdh::PdhNeeds {
            cpu: src.contains(DataSources::CPU),
            gpu: src.contains(DataSources::GPU),
            disk: src.contains(DataSources::DISK),
            thermal: src.contains(DataSources::SENSORS) && hw.thermal_zones,
        };
        if pdh.as_ref().map(|p| p.needs) != Some(needs) {
            pdh = if needs == pdh::PdhNeeds::default() { None } else { pdh::Pdh::open(needs) };
        }
        if let Some(p) = &pdh {
            p.collect();
        }

        let mut snap = Snapshot::default();
        if src.contains(DataSources::CPU) {
            // Performance counters can be disabled or corrupted; fall back to raw system times.
            snap.cpu_usage = pdh.as_ref().and_then(|p| p.cpu()).or_else(|| cpu_times.sample());
            snap.cpu_name = cache.cpu_name.clone();
        }
        if src.contains(DataSources::MEMORY)
            && let Some((used, total)) = system::memory()
        {
            snap.ram_used = used;
            snap.ram_total = total;
        }

        let want_gpu = src.contains(DataSources::GPU);
        let want_sensors = src.contains(DataSources::SENSORS);
        if (want_gpu || want_sensors) && stale(cache.adapters_at, Duration::from_secs(60)) {
            cache.adapters = gpu::adapters();
            cache.adapters_at = Some(Instant::now());
        }

        if want_sensors && stale(cache.sensors_at, Duration::from_millis(hw.sensor_interval_ms as u64)) {
            cache.sensors_at = Some(Instant::now());
            cache.gpu_sensors.clear();
            if hw.gpu_sensors {
                for a in &cache.adapters {
                    if let Some(s) = gpu_sensors.read(a) {
                        cache.gpu_sensors.insert(a.luid_str.clone(), s);
                    }
                }
            }
            if (hw.lhm_bridge || elevated) && wmi_client.is_none() {
                wmi_client = wmi::Wmi::new();
            }
            cache.sensors = match (&mut wmi_client, hw.lhm_bridge) {
                (Some(w), true) => w.lhm_sensors(),
                _ => Vec::new(),
            };
            cache.cpu_temp =
                wmi::cpu_temperature(&cache.sensors).or_else(|| pdh.as_ref().and_then(|p| p.thermal_celsius())).or_else(|| {
                    if elevated && hw.thermal_zones { wmi_client.as_mut().and_then(|w| w.acpi_temperature()) } else { None }
                });
        }

        if want_gpu || want_sensors {
            let usage = pdh.as_ref().and_then(|p| p.gpu_usage());
            let dedicated = pdh.as_ref().and_then(|p| p.gpu_dedicated());
            let default_luid = cache.adapters.iter().max_by_key(|a| a.dedicated).map(|a| a.luid_str.clone());
            snap.gpus = cache
                .adapters
                .iter()
                .map(|a| {
                    let sensor = cache.gpu_sensors.get(&a.luid_str);
                    GpuInfo {
                        name: a.name.clone(),
                        luid: a.luid_str.clone(),
                        usage: usage.as_ref().filter(|_| want_gpu).map(|u| u.get(&a.luid_str).copied().unwrap_or(0.0)),
                        vram_used: dedicated.as_ref().and_then(|d| d.get(&a.luid_str).copied()).unwrap_or(0),
                        // Without the usage counter the percentage would be meaningless.
                        vram_total: if dedicated.is_some() { a.dedicated } else { 0 },
                        temperature: sensor.and_then(|s| s.temperature),
                        fan_rpm: sensor.and_then(|s| s.fan_rpm),
                        is_default: default_luid.as_deref() == Some(a.luid_str.as_str()),
                    }
                })
                .collect();
        }
        if want_sensors {
            snap.cpu_temperature = cache.cpu_temp;
            snap.sensors = cache.sensors.iter().map(|s| (s.id.clone(), s.clone())).collect();
        }
        if src.contains(DataSources::NETWORK) {
            snap.networks = net.sample();
        }
        if src.contains(DataSources::DISK) {
            snap.disks = pdh.as_ref().map(|p| p.disks()).unwrap_or_default();
        }
        if src.contains(DataSources::DRIVES) {
            if stale(cache.drives_at, Duration::from_secs(15)) {
                cache.drives = system::drives();
                cache.drives_at = Some(Instant::now());
            }
            snap.drives = cache.drives.clone();
        }

        {
            let mut st = shared.state.lock().unwrap();
            snap.claude = st.claude.clone();
            st.snapshot = Some(snap);
        }
        shared.notify_ui();

        let interval = Duration::from_millis(req.interval_ms as u64);
        let deadline = started + interval;
        let mut st = shared.state.lock().unwrap();
        loop {
            if st.stop || st.generation != generation {
                break;
            }
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            st = shared.wake.wait_timeout(st, deadline - now).unwrap().0;
        }
    }
}
