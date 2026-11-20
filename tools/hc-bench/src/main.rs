//! hc-bench — resource measurements for Hyper Clips.
//!
//! Usage:
//!   hc-bench sample <exe-name> [seconds] [csv-path]
//!       Samples working set, private bytes and CPU% of every process with
//!       that image name once per second, prints min/avg/max.
//!   hc-bench spike-d3d11 [seconds]
//!       Measures this process's memory before/after creating a D3D11 device
//!       on each hardware adapter (driver user-mode cost, Phase 2 risk).

use std::io::Write;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use hc_core::sys::{self, MemoryUsage};
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ,
};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("sample") if args.len() >= 2 => {
            let secs = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(30);
            sample(&args[1], secs, args.get(3).map(String::as_str))
        }
        Some("spike-d3d11") => {
            let secs = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(5);
            spike_d3d11(secs)
        }
        _ => {
            eprintln!(
                "usage: hc-bench sample <exe-name> [seconds] [csv]\n       hc-bench spike-d3d11 [seconds]"
            );
            ExitCode::from(2)
        }
    }
}

const MB: f64 = 1024.0 * 1024.0;

#[derive(Default)]
struct Stats {
    n: u32,
    ws: (f64, f64, f64),
    pb: (f64, f64, f64),
    cpu: (f64, f64, f64),
}

impl Stats {
    fn add(&mut self, ws: f64, pb: f64, cpu: f64) {
        let upd = |t: &mut (f64, f64, f64), v: f64, first: bool| {
            if first {
                *t = (v, 0.0, v);
            }
            t.0 = t.0.min(v);
            t.1 += v;
            t.2 = t.2.max(v);
        };
        let first = self.n == 0;
        upd(&mut self.ws, ws, first);
        upd(&mut self.pb, pb, first);
        upd(&mut self.cpu, cpu, first);
        self.n += 1;
    }

    fn print(&self, name: &str) {
        let n = f64::from(self.n.max(1));
        println!("{name}: {} samples", self.n);
        println!(
            "  working set MB  min {:6.1}  avg {:6.1}  max {:6.1}",
            self.ws.0,
            self.ws.1 / n,
            self.ws.2
        );
        println!(
            "  private MB      min {:6.1}  avg {:6.1}  max {:6.1}",
            self.pb.0,
            self.pb.1 / n,
            self.pb.2
        );
        println!(
            "  CPU %           min {:6.2}  avg {:6.2}  max {:6.2}",
            self.cpu.0,
            self.cpu.1 / n,
            self.cpu.2
        );
    }
}

fn sample(exe: &str, secs: u64, csv: Option<&str>) -> ExitCode {
    let pids = sys::find_processes(exe);
    let Some(&pid) = pids.first() else {
        eprintln!("no process named {exe}");
        return ExitCode::FAILURE;
    };
    // SAFETY: handle closed at the end of the function.
    let Ok(handle) = (unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ,
            false,
            pid,
        )
    }) else {
        eprintln!("cannot open pid {pid}");
        return ExitCode::FAILURE;
    };
    let cpus = std::thread::available_parallelism().map_or(1, |n| n.get()) as f64;
    let mut out = csv.and_then(|p| std::fs::File::create(p).ok());
    if let Some(f) = out.as_mut() {
        let _ = writeln!(f, "t_s,working_set_mb,private_mb,cpu_pct");
    }
    let mut stats = Stats::default();
    let start = Instant::now();
    let mut last = (
        Instant::now(),
        sys::process_times(handle).map_or(0, |t| t.1),
    );
    for _ in 0..secs {
        std::thread::sleep(Duration::from_secs(1));
        let Some(mem) = sys::process_memory(handle) else {
            eprintln!("process exited");
            break;
        };
        let cpu_now = sys::process_times(handle).map_or(0, |t| t.1);
        let wall = last.0.elapsed().as_secs_f64();
        let cpu = (cpu_now.saturating_sub(last.1) as f64 / 1e7) / wall / cpus * 100.0;
        last = (Instant::now(), cpu_now);
        let (ws, pb) = (mem.working_set as f64 / MB, mem.private_bytes as f64 / MB);
        stats.add(ws, pb, cpu);
        if let Some(f) = out.as_mut() {
            let _ = writeln!(
                f,
                "{:.0},{ws:.2},{pb:.2},{cpu:.3}",
                start.elapsed().as_secs_f64()
            );
        }
    }
    // SAFETY: we own the handle.
    unsafe {
        let _ = CloseHandle(handle);
    }
    stats.print(&format!("{exe} (pid {pid})"));
    ExitCode::SUCCESS
}

fn show(label: &str, m: MemoryUsage) {
    println!(
        "{label:<48} working set {:6.1} MB   private {:6.1} MB",
        m.working_set as f64 / MB,
        m.private_bytes as f64 / MB
    );
}

fn spike_d3d11(hold_secs: u64) -> ExitCode {
    use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
    use windows::Win32::Graphics::Direct3D11::{
        D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, D3D11CreateDevice, ID3D11Device,
    };
    use windows::Win32::Graphics::Dxgi::{
        CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIAdapter1, IDXGIFactory1,
    };

    show("baseline", sys::current_memory());
    // SAFETY: COM-style calls; all interfaces are reference counted by windows-rs.
    let factory: IDXGIFactory1 = match unsafe { CreateDXGIFactory1() } {
        Ok(f) => f,
        Err(e) => {
            eprintln!("DXGI factory: {e}");
            return ExitCode::FAILURE;
        }
    };
    show("after DXGI factory", sys::current_memory());
    let mut devices: Vec<ID3D11Device> = Vec::new();
    for i in 0.. {
        // SAFETY: EnumAdapters1 returns DXGI_ERROR_NOT_FOUND past the end.
        let Ok(adapter): windows::core::Result<IDXGIAdapter1> =
            (unsafe { factory.EnumAdapters1(i) })
        else {
            break;
        };
        // SAFETY: plain descriptor query.
        let Ok(desc) = (unsafe { adapter.GetDesc1() }) else {
            continue;
        };
        if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
            continue;
        }
        let len = desc.Description.iter().position(|&c| c == 0).unwrap_or(128);
        let name = String::from_utf16_lossy(&desc.Description[..len]);
        let mut device = None;
        // SAFETY: valid adapter; out-param receives the device.
        let res = unsafe {
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                Default::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                None,
            )
        };
        match (res, device) {
            (Ok(()), Some(d)) => {
                devices.push(d);
                show(
                    &format!("after D3D11 device on {name}"),
                    sys::current_memory(),
                );
            }
            (Err(e), _) => println!("{name}: device creation failed: {e}"),
            _ => {}
        }
    }
    std::thread::sleep(Duration::from_secs(hold_secs));
    show(
        &format!("after holding {hold_secs}s"),
        sys::current_memory(),
    );
    drop(devices);
    show("after releasing devices", sys::current_memory());
    ExitCode::SUCCESS
}
