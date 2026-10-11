mod backgrounds;
mod comfy;
mod commands;
mod env;
mod error;
mod media;
mod project;
mod worker;
#[cfg(test)]
mod e2e;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(commands::AppState::new())
        .invoke_handler(tauri::generate_handler![
            commands::check_environment,
            commands::probe_video,
            commands::create_project,
            commands::open_project,
            commands::dev_autoload,
            commands::dev_demo,
            commands::dev_log,
            commands::close_project,
            commands::set_clip,
            commands::prepare_proxy,
            commands::set_prompts,
            commands::preview_mask,
            commands::propagate_mask,
            commands::cancel_job,
            commands::unload_models,
            commands::set_background,
            commands::select_background,
            commands::list_backgrounds,
            commands::delete_background,
            commands::image_backends,
            commands::set_generation,
            commands::generate_backgrounds,
            commands::set_composite,
            commands::export_video,
            commands::project_path,
            commands::mask_quality,
            commands::set_cleanup,
            commands::set_export_quality,
            commands::disk_usage,
            commands::clean_cache,
        ])
        .on_window_event(|window, event| {
            // Fermer la fenêtre arrête proprement le worker Python.
            if let tauri::WindowEvent::Destroyed = event {
                let state = window.state::<commands::AppState>();
                let workers = [state.worker.lock().unwrap().take(), state.image_worker.lock().unwrap().take()];
                for w in workers.into_iter().flatten() {
                    w.shutdown();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("erreur au lancement de videeo");
}
