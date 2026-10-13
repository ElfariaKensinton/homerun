mod client;
mod commands;
mod remote_access;
mod tray;
mod window;

use client::DaemonClient;
use tauri::menu::{AboutMetadata, MenuBuilder, MenuItem, SubmenuBuilder};
use tauri::Emitter;
use tokio::sync::Mutex;

pub struct AppState {
    pub client: Mutex<DaemonClient>,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let client = DaemonClient::default_socket();

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_positioner::init())
        .manage(AppState {
            client: Mutex::new(client),
        })
        .setup(|app| {
            // -- Build menu --
            let check_updates = MenuItem::with_id(
                app,
                "check_updates",
                "Check for Updates...",
                true,
                None::<&str>,
            )?;
            let settings = MenuItem::with_id(
                app,
                "settings",
                "Settings...",
                true,
                Some("CmdOrCtrl+,"),
            )?;

            let about_metadata = AboutMetadata {
                name: Some("HomeRun".into()),
                version: Some(env!("CARGO_PKG_VERSION").into()),
                copyright: Some("© 2026 HomeRun contributors".into()),
                credits: Some("Manage GitHub Actions self-hosted runners".into()),
                ..Default::default()
            };

            let app_submenu = SubmenuBuilder::new(app, "HomeRun")
                .about(Some(about_metadata))
                .item(&check_updates)
                .separator()
                .item(&settings)
                .separator()
                .hide()
                .hide_others()
                .show_all()
                .separator()
                .quit()
                .build()?;

            let edit_submenu = SubmenuBuilder::new(app, "Edit")
                .copy()
                .paste()
                .select_all()
                .build()?;

            let toggle_mini = MenuItem::with_id(
                app,
                "toggle_mini",
                "Toggle Mini View",
                true,
                Some("CmdOrCtrl+Shift+M"),
            )?;

            let window_submenu = SubmenuBuilder::new(app, "Window")
                .minimize()
                .item(&toggle_mini)
                .separator()
                .close_window()
                .build()?;

            let github_item = MenuItem::with_id(
                app,
                "open_github",
                "HomeRun on GitHub",
                true,
                None::<&str>,
            )?;
            let report_issue = MenuItem::with_id(
                app,
                "report_issue",
                "Report an Issue...",
                true,
                None::<&str>,
            )?;

            let help_submenu = SubmenuBuilder::new(app, "Help")
                .item(&github_item)
                .item(&report_issue)
                .build()?;

            let menu = MenuBuilder::new(app)
                .item(&app_submenu)
                .item(&edit_submenu)
                .item(&window_submenu)
                .item(&help_submenu)
                .build()?;

            app.set_menu(menu)?;

            // -- Handle custom menu events --
            app.on_menu_event(move |app_handle, event| {
                use tauri_plugin_opener::OpenerExt;
                match event.id().as_ref() {
                    "check_updates" => {
                        let _ = app_handle.opener().open_url(
                            "https://github.com/aGallea/homerun/releases",
                            None::<&str>,
                        );
                    }
                    "toggle_mini" => {
                        let _ = crate::window::toggle_mini_window(app_handle);
                    }
                    "settings" => {
                        let _ = app_handle.emit("navigate", "/settings");
                    }
                    "open_github" => {
                        let _ = app_handle.opener().open_url(
                            "https://github.com/aGallea/homerun",
                            None::<&str>,
                        );
                    }
                    "report_issue" => {
                        let _ = app_handle.opener().open_url(
                            "https://github.com/aGallea/homerun/issues/new/choose",
                            None::<&str>,
                        );
                    }
                    _ => {}
                }
            });

            // -- Spawn bundled daemon with authenticated LAN access --
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let remote_token = match crate::remote_access::ensure_token() {
                    Ok(token) => token,
                    Err(error) => {
                        eprintln!("Failed to initialize HomeRun LAN API token: {error}");
                        return;
                    }
                };

                let client = crate::client::DaemonClient::default_socket();
                if client.health().await.is_ok() {
                    // Do not interrupt an existing daemon on app launch. Settings
                    // provides an explicit restart action to apply listener settings.
                    return;
                }

                use tauri_plugin_shell::ShellExt;
                let sidecar = match handle.shell().sidecar("homerund") {
                    Ok(command) => command
                        .env("HOMERUN_REMOTE_BIND", crate::remote_access::BIND_ADDRESS)
                        .env("HOMERUN_REMOTE_TOKEN", remote_token),
                    Err(error) => {
                        eprintln!("Failed to find homerund sidecar: {error}");
                        return;
                    }
                };
                match sidecar.spawn() {
                    Ok(_) => eprintln!("HomeRun daemon spawned with LAN API on {}", crate::remote_access::BIND_ADDRESS),
                    Err(error) => eprintln!("Failed to spawn daemon: {error}"),
                }
            });

            // -- Set up macOS notifications --
            // In dev mode, impersonate Terminal so notifications are delivered.
            // In production, use the app's own bundle identifier.
            #[cfg(target_os = "macos")]
            {
                let bundle_id = if tauri::is_dev() {
                    "com.apple.Terminal"
                } else {
                    "com.homerun.app"
                };
                let _ = mac_notification_sys::set_application(bundle_id);
            }

            // -- Initialize system tray --
            if let Err(e) = tray::init(app) {
                eprintln!("Failed to initialize tray: {e}");
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::health_check,
            commands::remote_access_info,
            commands::daemon_available,
            commands::start_daemon,
            commands::stop_daemon,
            commands::restart_daemon,
            commands::list_runners,
            commands::create_runner,
            commands::delete_runner,
            commands::start_runner,
            commands::stop_runner,
            commands::restart_runner,
            commands::auth_status,
            commands::login_with_token,
            commands::logout,
            commands::start_device_flow,
            commands::poll_device_flow,
            commands::list_repos,
            commands::get_metrics,
            commands::docker_status,
            commands::service_status,
            commands::install_service,
            commands::uninstall_service,
            commands::get_runner_logs,
            commands::create_batch,
            commands::start_group,
            commands::stop_group,
            commands::restart_group,
            commands::delete_group,
            commands::scale_group,
            commands::get_preferences,
            commands::update_preferences,
            commands::scan_local,
            commands::scan_remote,
            commands::start_scan,
            commands::cancel_scan,
            commands::get_scan_results,
            commands::get_daemon_logs_recent,
            commands::get_runner_steps,
            commands::get_step_logs,
            commands::get_runner_history,
            commands::rerun_workflow,
            commands::clear_runner_history,
            commands::delete_history_entry,
            commands::update_tray_icon,
            commands::toggle_mini_window,
            commands::show_main_window,
            commands::hide_all_windows,
            commands::save_mini_position,
            commands::get_mini_position,
            commands::quit_app,
            commands::send_notification,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
