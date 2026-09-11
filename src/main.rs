mod ui;

fn main() -> eframe::Result {
    let started = std::time::Instant::now();
    let demo = std::env::args().any(|arg| arg == "--demo");
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title(if demo { "Qrow · Demo" } else { "Qrow" })
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([850.0, 560.0]),
        centered: true,
        ..Default::default()
    };
    eframe::run_native(
        "Qrow",
        options,
        Box::new(move |cc| Ok(Box::new(ui::Qrow::new(cc, demo, started)))),
    )
}
