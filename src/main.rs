use std::sync::OnceLock;

use gio::prelude::*;
use gtk::glib;

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

    // Suppress "Theme parser error" GTK warnings emitted when the host system
    // theme contains CSS features our bundled GTK (AppImage) doesn't recognise.
    // GTK4 routes these through GLib's structured logging path, so we must use
    // log_set_writer_func — log_set_handler only catches old-style g_log() calls.
    glib::log_set_writer_func(|level, fields| {
        let domain = fields
            .iter()
            .find(|f| f.key() == "GLIB_DOMAIN")
            .and_then(|f| f.value_str())
            .unwrap_or("");
        let message = fields
            .iter()
            .find(|f| f.key() == "MESSAGE")
            .and_then(|f| f.value_str())
            .unwrap_or("");

        if domain == "Gtk"
            && level == glib::LogLevel::Warning
            && message.contains("Theme parser error")
        {
            return glib::LogWriterOutput::Handled;
        }

        glib::log_writer_default(level, fields)
    });

    // Initialize GTK and libadwaita (required when using RelmApp::from_app)
    gtk::init().expect("GTK initialisation failed — no display server available");
    libadwaita::init().expect("libadwaita initialisation failed — no display server available");

    #[cfg(not(feature = "experimental"))]
    glib::log_set_default_handler(|domain, level, message| {
        if matches!(level, glib::LogLevel::Error | glib::LogLevel::Critical) {
            eprintln!(
                "({}): {:?}: {}",
                domain.unwrap_or("unknown"),
                level,
                message
            );
        }
    });

    // Register bundled icons so themes that lack notification-symbolic use ours.
    // GTK looks for icons at {prefix}/{size}/{context}/{name}.svg, so the prefix
    // must be the parent of the scalable/ directory inside the gresource.
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::IconTheme::for_display(&display).add_resource_path("/io/mattianelo/Deployd/icons");
    }

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
            0%   { opacity: 1; }
            50%  { opacity: 0.4; }
            100% { opacity: 1; }
        }
        .notification-active {
            color: @error_color;
            animation: notification-pulse 1.5s ease-in-out infinite;
        }
        .notification-badge {
            font-size: 10px;
            font-weight: 700;
            color: @error_color;
        }
        .filter-chip {
            padding: 2px 10px;
            min-height: 0;
            font-size: 0.85em;
            transition: background-color 150ms ease, color 150ms ease;
        }
        .compact-row {
            min-height: 0;
        }
        .install-action-btn {
            padding: 4px 14px;
            min-height: 0;
            font-size: 0.9em;
        }
        .linked > button, .linked > menubutton > button {
            transition: background-color 200ms ease;
        }
        .linked > menubutton.suggested-action {
            background: transparent;
            box-shadow: none;
        }
        .linked > menubutton.suggested-action > button {
            background: @accent_bg_color;
            color: @accent_fg_color;
            border-radius: 0 9999px 9999px 0;
        }
        .plugin-badge {
            font-size: 10px;
            font-weight: 700;
            padding: 1px 5px;
            border-radius: 4px;
            letter-spacing: 0.03em;
        }
        .plugin-badge-esm {
            background-color: alpha(@accent_color, 0.2);
            color: @accent_color;
        }
        .plugin-badge-esl {
            background-color: alpha(@success_color, 0.2);
            color: @success_color;
        }
        .plugin-badge-esp {
            background-color: alpha(@warning_color, 0.15);
            color: @warning_color;
        }
        row checkbutton check {
            transition: opacity 150ms ease;
        }
        .group-color-dot {
            min-width: 10px;
            min-height: 10px;
            border-radius: 5px;
        }
        .color-swatch {
            min-width: 18px;
            min-height: 18px;
            padding: 0;
            border-radius: 9px;
        }
        .color-red, .color-swatch.red     { background-color: #e53935; }
        .color-orange, .color-swatch.orange { background-color: #fb8c00; }
        .color-yellow, .color-swatch.yellow { background-color: #fdd835; }
        .color-green, .color-swatch.green  { background-color: #43a047; }
        .color-teal, .color-swatch.teal   { background-color: #00897b; }
        .color-blue, .color-swatch.blue   { background-color: #1e88e5; }
        .color-purple, .color-swatch.purple { background-color: #8e24aa; }
        .color-pink, .color-swatch.pink   { background-color: #d81b60; }
        .mod-separator-row { transition: background-color 150ms ease; }
        .mod-separator-row button { opacity: 0; transition: opacity 150ms ease; }
        .mod-separator-row:hover button { opacity: 1; }
        .mod-row button { opacity: 0; transition: opacity 150ms ease; }
        .mod-row:hover button { opacity: 1; }
        .mod-row-enabled { background-color: alpha(@accent_color, 0.08); border-radius: 6px; }
        .code-pill { font-family: monospace; background-color: alpha(@window_fg_color, 0.07); border-radius: 4px; padding: 1px 6px; }
        ",
    );

    utils::nxm_handler::ensure_registered();

    app.run::<app::App>(None);
}
