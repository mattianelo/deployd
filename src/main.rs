use std::sync::OnceLock;

use gio::prelude::*;

mod app;
mod core;
mod models;
mod ui;
mod utils;

/// Global sender for forwarding NXM links to the running App component.
/// Set once during Component::init(), used by the command-line signal handler.
pub static NXM_SENDER: OnceLock<relm4::Sender<app::AppMsg>> = OnceLock::new();

fn main() {
    gio::resources_register_include!("resources.gresource")
        .expect("failed to register app resources");

    // Initialize GTK and libadwaita (required when using RelmApp::from_app)
    gtk::init().unwrap();
    libadwaita::init().unwrap();

    let gtk_app = libadwaita::Application::builder()
        .application_id("app.deployd")
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();

    // Handle command-line arguments for both first and subsequent invocations.
    // When the app is already running and a second `deployd nxm://...` is launched,
    // GTK forwards the args to the running instance via this signal.
    gtk_app.connect_command_line(|app, cmdline| {
        let args = cmdline.arguments();
        for arg in args.iter().skip(1) {
            let s = arg.to_string_lossy();
            if s.starts_with("nxm://") {
                if let Some(sender) = NXM_SENDER.get() {
                    let _ = sender.send(app::AppMsg::NxmLinkReceived(s.to_string()));
                }
                break;
            }
        }

        app.activate();
        0.into()
    });

    let app = relm4::RelmApp::from_app(gtk_app);

    relm4::set_global_css(
        "
        .drop-above {
            border-top: 3px solid @accent_color;
        }
        .drop-below {
            border-bottom: 3px solid @accent_color;
        }
        @keyframes notification-pulse {
            0%, 100% { opacity: 1; }
            50% { opacity: 0.4; }
        }
        .notification-active {
            color: @warning_color;
            animation: notification-pulse 1.5s ease-in-out infinite;
        }
        ",
    );

    utils::nxm_handler::ensure_registered();

    app.run::<app::App>(None);
}
