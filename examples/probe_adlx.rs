// Probe descartável: confirma que o ADLX inicializa e lê métricas nesta máquina.
fn main() -> anyhow::Result<()> {
    let helper = adlx::helper::AdlxHelper::new()?;
    let sys = helper.system();
    let pms = sys.performance_monitoring_services()?;
    let gpus = sys.gpus()?;
    for gpu in gpus.iter() {
        use adlx::Interface; let g1: adlx::Gpu1 = gpu.cast()?;
        println!(
            "gpu {} ext={:?} pnp={:?} bus={:?} lanes={:?} vram={:?}",
            gpu.name()?,
            gpu.is_external(),
            gpu.pnp_string(),
            g1.pci_bus_type(),
            g1.pci_bus_lane_width(),
            gpu.total_vram()
        );
        let m = pms.current_gpu_metrics(&gpu)?;
        println!(
            "  temp={:?} hot={:?} pwr={:?} tbp={:?} clk={:?} vclk={:?} fan={:?} vram={:?} use={:?} volt={:?}",
            m.temperature(),
            m.hotspot_temperature(),
            m.power(),
            m.total_board_power(),
            m.clock_speed(),
            m.vram_clock_speed(),
            m.fan_speed(),
            m.vram(),
            m.usage(),
            m.voltage()
        );
    }
    Ok(())
}
