//! NVML (driver NVIDIA): `nvml.dll` carregada dinamicamente, só do System32 e só se a GPU do dock
//! for NVIDIA. Sem placa NVIDIA (ou sem o driver), nada é carregado e o painel segue com DXGI/PDH.
//! O que o NVML não expõe aqui (ponto mais quente, voltagem, ventoinha em RPM) fica `None`.
//!
//! Não testado em placa física: escrito pela API pública do NVML (nvml.h), sem hardware NVIDIA no
//! setup de desenvolvimento.

use crate::collect::GpuM;
use std::ffi::c_void;
use windows::core::{s, w, PCSTR};
use windows::Win32::Foundation::{FreeLibrary, HMODULE};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32};

type Dev = *mut c_void;
type Ret = i32; // nvmlReturn_t; 0 = NVML_SUCCESS

#[repr(C)]
struct PciInfo {
    bus_id_legacy: [u8; 16],
    domain: u32,
    bus: u32,
    device: u32,
    /// (Device ID << 16) | Vendor ID
    pci_device_id: u32,
    pci_subsystem_id: u32,
    bus_id: [u8; 32],
}

#[repr(C)]
struct Util {
    gpu: u32,
    memory: u32,
}

#[repr(C)]
struct Mem {
    total: u64,
    free: u64,
    used: u64,
}

const CLOCK_GRAPHICS: u32 = 0;
const CLOCK_MEM: u32 = 2;
const TEMP_GPU: u32 = 0;

pub struct Nvml {
    lib: HMODULE,
    dev: Dev,
    shutdown: unsafe extern "C" fn() -> Ret,
    temp: unsafe extern "C" fn(Dev, u32, *mut u32) -> Ret,
    power: unsafe extern "C" fn(Dev, *mut u32) -> Ret,
    clock: unsafe extern "C" fn(Dev, u32, *mut u32) -> Ret,
    util: unsafe extern "C" fn(Dev, *mut Util) -> Ret,
    mem: unsafe extern "C" fn(Dev, *mut Mem) -> Ret,
    pub name: String,
    pub vram_total_mb: u32,
}

// O handle do NVML é usado só pela thread do coletor.
unsafe impl Send for Nvml {}

impl Drop for Nvml {
    fn drop(&mut self) {
        unsafe {
            (self.shutdown)();
            let _ = FreeLibrary(self.lib);
        }
    }
}

/// `pciDeviceId` do NVML para um par Vendor/Device ID do PnP.
pub fn pci_device_id(ven: u16, dev: u16) -> u32 {
    (dev as u32) << 16 | ven as u32
}

unsafe fn sym<T>(lib: HMODULE, name: PCSTR) -> anyhow::Result<T> {
    let f = unsafe { GetProcAddress(lib, name) }.ok_or_else(|| anyhow::anyhow!("nvml.dll: {}", unsafe { name.display() }))?;
    Ok(unsafe { std::mem::transmute_copy(&f) })
}

impl Nvml {
    /// Abre o NVML e acha a placa de mesmo Vendor/Device ID da GPU do dock.
    pub fn open(ven: u16, dev: u16) -> anyhow::Result<Self> {
        unsafe {
            let lib = LoadLibraryExW(w!("nvml.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32)?;
            let r = Self::bind(lib, pci_device_id(ven, dev));
            if r.is_err() {
                let _ = FreeLibrary(lib);
            }
            r
        }
    }

    unsafe fn bind(lib: HMODULE, want: u32) -> anyhow::Result<Self> {
        unsafe {
            let init: unsafe extern "C" fn() -> Ret = sym(lib, s!("nvmlInit_v2"))?;
            let shutdown: unsafe extern "C" fn() -> Ret = sym(lib, s!("nvmlShutdown"))?;
            let count: unsafe extern "C" fn(*mut u32) -> Ret = sym(lib, s!("nvmlDeviceGetCount_v2"))?;
            let by_index: unsafe extern "C" fn(u32, *mut Dev) -> Ret = sym(lib, s!("nvmlDeviceGetHandleByIndex_v2"))?;
            let pci: unsafe extern "C" fn(Dev, *mut PciInfo) -> Ret = sym(lib, s!("nvmlDeviceGetPciInfo_v3"))?;
            let get_name: unsafe extern "C" fn(Dev, *mut u8, u32) -> Ret = sym(lib, s!("nvmlDeviceGetName"))?;
            let mem: unsafe extern "C" fn(Dev, *mut Mem) -> Ret = sym(lib, s!("nvmlDeviceGetMemoryInfo"))?;
            let r = init();
            if r != 0 {
                anyhow::bail!("nvmlInit_v2 = {r}");
            }
            let fail = |m: String| {
                shutdown();
                anyhow::anyhow!(m)
            };
            let mut n = 0u32;
            if count(&mut n) != 0 {
                return Err(fail("nvmlDeviceGetCount_v2".into()));
            }
            let mut found = None;
            for i in 0..n {
                let mut d: Dev = std::ptr::null_mut();
                let mut p: PciInfo = std::mem::zeroed();
                if by_index(i, &mut d) == 0 && pci(d, &mut p) == 0 && p.pci_device_id == want {
                    found = Some(d);
                    break;
                }
            }
            let Some(dev) = found else {
                return Err(fail(crate::i18n::t("placa do dock não listada pelo NVML", "dock card not listed by NVML").into()));
            };
            let mut buf = [0u8; 96];
            let name = if get_name(dev, buf.as_mut_ptr(), buf.len() as u32) == 0 {
                String::from_utf8_lossy(&buf[..buf.iter().position(|&b| b == 0).unwrap_or(buf.len())]).into_owned()
            } else {
                String::new()
            };
            let mut m: Mem = std::mem::zeroed();
            let vram_total_mb = if mem(dev, &mut m) == 0 { (m.total / 1048576) as u32 } else { 0 };
            Ok(Self {
                lib,
                dev,
                shutdown,
                temp: sym(lib, s!("nvmlDeviceGetTemperature"))?,
                power: sym(lib, s!("nvmlDeviceGetPowerUsage"))?,
                clock: sym(lib, s!("nvmlDeviceGetClockInfo"))?,
                util: sym(lib, s!("nvmlDeviceGetUtilizationRates"))?,
                mem,
                name,
                vram_total_mb,
            })
        }
    }

    /// Uma leitura. Cada campo que o NVML recusar fica `None`; se nada vier, a placa sumiu.
    pub fn read(&mut self) -> anyhow::Result<GpuM> {
        unsafe {
            let u32_of = |f: &dyn Fn(*mut u32) -> Ret| {
                let mut v = 0u32;
                (f(&mut v) == 0).then_some(v)
            };
            let temp = u32_of(&|p| (self.temp)(self.dev, TEMP_GPU, p)).map(|v| v as f64);
            let tbp = u32_of(&|p| (self.power)(self.dev, p)).map(|mw| mw as f64 / 1000.0);
            let clk = u32_of(&|p| (self.clock)(self.dev, CLOCK_GRAPHICS, p)).map(|v| v as i32);
            let vclk = u32_of(&|p| (self.clock)(self.dev, CLOCK_MEM, p)).map(|v| v as i32);
            let mut ut = Util { gpu: 0, memory: 0 };
            let usage = ((self.util)(self.dev, &mut ut) == 0).then_some(ut.gpu as f64);
            let mut m: Mem = std::mem::zeroed();
            let vram = ((self.mem)(self.dev, &mut m) == 0).then_some((m.used / 1048576) as i32);
            if temp.is_none() && tbp.is_none() && clk.is_none() && usage.is_none() && vram.is_none() {
                anyhow::bail!("NVML: no reply");
            }
            Ok(GpuM { temp, hot: None, tbp, clk, vclk, fan: None, vram, usage, volt: None })
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn pci_device_id_combina_device_e_vendor() {
        // RTX 4090: VEN_10DE&DEV_2684 → 0x268410DE, como o NVML reporta em pciDeviceId
        assert_eq!(super::pci_device_id(0x10DE, 0x2684), 0x2684_10DE);
    }
}
