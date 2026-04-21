#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    AppHandle, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent,
};

fn main() {
    cm_app::init_tracing();

    let ui_dir = cm_app::locate_ui_dir();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let daemon = rt
        .block_on(cm_app::start(ui_dir))
        .expect("start cm-app daemon");
    let port = daemon.port;

    // Keep the runtime and daemon alive for the life of the process.
    // We don't currently wire shutdown through Tauri's exit hook — the OS
    // reaps the sockets on process exit, and the tailer is fs-watcher only.
    std::mem::forget(daemon.shutdown_tx);
    std::mem::forget(daemon.join);
    std::mem::forget(rt);

    let url = format!("http://127.0.0.1:{}/", port);

    tauri::Builder::default()
        .setup(move |app| {
            build_main_window(&app.handle(), &url)?;
            build_tray(&app.handle())?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                // Hide instead of quit so the daemon keeps running in the tray.
                if window.label() == "main" {
                    let _ = window.hide();
                    api.prevent_close();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

fn build_main_window(app: &AppHandle, url: &str) -> tauri::Result<()> {
    let parsed = url.parse().expect("valid url");
    WebviewWindowBuilder::new(app, "main", WebviewUrl::External(parsed))
        .title("Claude Monitor")
        .inner_size(1200.0, 800.0)
        .min_inner_size(720.0, 480.0)
        .build()?;
    Ok(())
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Show", true, None::<&str>)?;
    let hide = MenuItem::with_id(app, "hide", "Hide", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &hide, &quit])?;

    TrayIconBuilder::with_id("main-tray")
        .tooltip("Claude Monitor")
        .icon(app.default_window_icon().cloned().expect("window icon"))
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main(app),
            "hide" => {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.hide();
                }
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let tauri::tray::TrayIconEvent::Click {
                button: tauri::tray::MouseButton::Left,
                button_state: tauri::tray::MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}
