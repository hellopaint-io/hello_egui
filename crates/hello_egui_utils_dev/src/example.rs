use eframe::{Frame, NativeOptions};
use egui::{CentralPanel, Ui};

/// Run an example with the given name and content.
pub fn run(name: &str, mut f: impl FnMut(&mut Ui) + 'static) {
    run_with_frame(name, move |ui, _frame| f(ui));
}

/// Like [`run`], but the content also gets the [`Frame`].
///
/// Some examples need what is on it, such as the wgpu render state.
pub fn run_with_frame(name: &str, mut f: impl FnMut(&mut Ui, &mut Frame) + 'static) {
    let mut initialized = false;
    eframe::run_ui_native(name, NativeOptions::default(), move |ui, frame| {
        if !initialized {
            initialized = true;
            return;
        }
        CentralPanel::default().show(ui, |ui| {
            let mut style = (*ui.ctx().global_style()).clone();
            ui.checkbox(&mut style.debug.debug_on_hover, "Debug on hover");
            ui.checkbox(&mut style.visuals.dark_mode, "Dark mode");
            ui.ctx().set_global_style(style);

            f(ui, frame);
        });
    })
    .unwrap();
}

/// Run an example with the given content.
#[macro_export]
macro_rules! run {
    ($content:expr) => {
        $crate::run(file!(), $content);
    };
}

/// Run an example whose content also gets the [`eframe::Frame`].
#[macro_export]
macro_rules! run_with_frame {
    ($content:expr) => {
        $crate::run_with_frame(file!(), $content);
    };
}
