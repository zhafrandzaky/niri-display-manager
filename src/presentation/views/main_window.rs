//! Widget construction for the main window.
//!
//! The UI thread only builds widgets and dispatches [`AppAction`] values into
//! an `async-channel`. A dedicated worker thread owns the [`DisplayService`]
//! and publishes [`AppEvent`] values back through `glib::spawn_future_local`,
//! so no blocking I/O ever runs on the main context.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use gtk4 as gtk;

use crate::APP_ID;
use crate::infrastructure::drm_sysfs::SysfsProbe;
use crate::infrastructure::niri_ipc::NiriCliClient;
use crate::infrastructure::process_runner::{MirrorPidFile, SystemCommandRunner, SystemSupervisor};
use crate::presentation::view_model::{self, AppAction, AppEvent, AppState, InfoRow, ProfileRow};
use crate::service::backup_service::FileConfigStore;
use crate::service::display_service::DisplayService;

type Service = DisplayService<NiriCliClient, SysfsProbe, FileConfigStore, SystemSupervisor>;

/// Run the GTK application until the window closes.
pub fn run() -> anyhow::Result<glib::ExitCode> {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_ui);
    Ok(app.run())
}

struct Ui {
    window: adw::ApplicationWindow,
    toast_overlay: adw::ToastOverlay,
    status_row: adw::ActionRow,
    profiles_group: adw::PreferencesGroup,
    outputs_group: adw::PreferencesGroup,
    ports_group: adw::PreferencesGroup,
    mirror_group: adw::PreferencesGroup,
    spinner: adw::Spinner,
    refresh_button: gtk::Button,
    state: RefCell<AppState>,
    profile_widgets: RefCell<Vec<gtk::Widget>>,
    output_widgets: RefCell<Vec<gtk::Widget>>,
    port_widgets: RefCell<Vec<gtk::Widget>>,
    actions: async_channel::Sender<AppAction>,
}

fn build_ui(app: &adw::Application) {
    let (action_sender, action_receiver) = async_channel::unbounded::<AppAction>();
    let (event_sender, event_receiver) = async_channel::unbounded::<AppEvent>();

    let profiles_group = adw::PreferencesGroup::builder()
        .title("Display Profiles")
        .build();
    let outputs_group = adw::PreferencesGroup::builder().title("Outputs").build();
    let ports_group = adw::PreferencesGroup::builder()
        .title("Hardware Ports")
        .build();
    let mirror_group = adw::PreferencesGroup::builder().title("Mirroring").build();

    let status_row = adw::ActionRow::builder()
        .title("Status")
        .subtitle("No managed profile applied")
        .build();
    status_row.set_activatable(false);
    status_row.add_prefix(&gtk::Image::from_icon_name("dialog-information-symbolic"));
    let status_group = adw::PreferencesGroup::new();
    status_group.add(&status_row);

    let stop_button = gtk::Button::builder()
        .label("Stop Mirroring")
        .tooltip_text("Terminate the wl-mirror process")
        .build();
    stop_button.add_css_class("destructive-action");
    {
        let actions = action_sender.clone();
        stop_button.connect_clicked(move |_| {
            let _ = actions.try_send(AppAction::StopMirror);
        });
    }
    let mirror_row = adw::ActionRow::builder()
        .title("Mirroring is running")
        .subtitle("The internal panel is mirrored to the external display")
        .build();
    mirror_row.set_activatable(false);
    mirror_row.add_prefix(&gtk::Image::from_icon_name("display-projector-symbolic"));
    mirror_row.add_suffix(&stop_button);
    mirror_group.add(&mirror_row);
    mirror_group.set_visible(false);

    let page = adw::PreferencesPage::new();
    page.add(&status_group);
    page.add(&profiles_group);
    page.add(&outputs_group);
    page.add(&ports_group);
    page.add(&mirror_group);

    let spinner = adw::Spinner::new();
    spinner.set_visible(false);
    let refresh_button = gtk::Button::builder()
        .icon_name("view-refresh-symbolic")
        .tooltip_text("Refresh display state")
        .build();
    {
        let actions = action_sender.clone();
        refresh_button.connect_clicked(move |_| {
            let _ = actions.try_send(AppAction::Refresh);
        });
    }

    let header = adw::HeaderBar::new();
    header.pack_start(&spinner);
    header.pack_end(&refresh_button);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&page));

    let toast_overlay = adw::ToastOverlay::new();
    toast_overlay.set_child(Some(&toolbar));

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Niri Display Manager")
        .default_width(520)
        .default_height(760)
        .content(&toast_overlay)
        .build();

    let ui = Rc::new(Ui {
        window: window.clone(),
        toast_overlay,
        status_row,
        profiles_group,
        outputs_group,
        ports_group,
        mirror_group,
        spinner,
        refresh_button,
        state: RefCell::new(AppState::default()),
        profile_widgets: RefCell::new(Vec::new()),
        output_widgets: RefCell::new(Vec::new()),
        port_widgets: RefCell::new(Vec::new()),
        actions: action_sender,
    });

    window.connect_close_request({
        let actions = ui.actions.clone();
        move |_| {
            if actions.try_send(AppAction::Shutdown).is_err() {
                // The worker is already gone; let the window close normally.
                glib::Propagation::Proceed
            } else {
                glib::Propagation::Stop
            }
        }
    });

    rebuild(&ui);
    window.present();

    spawn_worker(action_receiver, event_sender);
    install_event_handler(ui, event_receiver);
}

fn spawn_worker(
    action_receiver: async_channel::Receiver<AppAction>,
    event_sender: async_channel::Sender<AppEvent>,
) {
    let service = build_service();
    let spawn_result = std::thread::Builder::new()
        .name("niri-display-manager-worker".to_owned())
        .spawn(move || worker_loop(service, action_receiver, event_sender));
    if let Err(error) = spawn_result {
        log::error!("failed to spawn worker thread: {error}");
    }
}

fn build_service() -> Service {
    DisplayService::new(
        NiriCliClient::new(std::sync::Arc::new(SystemCommandRunner)),
        SysfsProbe::system(),
        FileConfigStore::from_environment(),
        SystemSupervisor::new(),
        MirrorPidFile::new(mirror_pid_path()),
    )
}

fn mirror_pid_path() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("niri-display-manager.mirror.pid")
}

fn worker_loop(
    service: Service,
    action_receiver: async_channel::Receiver<AppAction>,
    event_sender: async_channel::Sender<AppEvent>,
) {
    if let Err(error) = service.initialize() {
        send_event(
            &event_sender,
            AppEvent::Failed(format!("Failed to initialize: {error}")),
        );
    }
    publish_state(&service, &event_sender);

    while let Ok(action) = action_receiver.recv_blocking() {
        match action {
            AppAction::Shutdown => {
                if let Err(error) = service.shutdown() {
                    log::error!("shutdown failed: {error}");
                }
                send_event(&event_sender, AppEvent::ShutdownComplete);
                break;
            }
            AppAction::Refresh => publish_state(&service, &event_sender),
            AppAction::StopMirror => {
                send_event(&event_sender, AppEvent::Busy(true));
                match service.stop_mirror() {
                    Ok(()) => send_event(
                        &event_sender,
                        AppEvent::Notice("Mirroring stopped".to_owned()),
                    ),
                    Err(error) => send_event(
                        &event_sender,
                        AppEvent::Failed(format!("Failed to stop mirroring: {error}")),
                    ),
                }
                send_event(&event_sender, AppEvent::Busy(false));
                publish_state(&service, &event_sender);
            }
            AppAction::ApplyProfile(profile) => {
                send_event(&event_sender, AppEvent::Busy(true));
                match service.apply_profile(profile) {
                    Ok(report) => send_event(&event_sender, AppEvent::Applied(Box::new(report))),
                    Err(error) => send_event(
                        &event_sender,
                        AppEvent::Failed(format!("Failed to apply profile: {error}")),
                    ),
                }
                send_event(&event_sender, AppEvent::Busy(false));
                publish_state(&service, &event_sender);
            }
        }
        if event_sender.is_closed() {
            break;
        }
    }
}

fn publish_state(service: &Service, event_sender: &async_channel::Sender<AppEvent>) {
    match service.refresh_state() {
        Ok(state) => send_event(event_sender, AppEvent::State(Box::new(state))),
        Err(error) => send_event(
            event_sender,
            AppEvent::Failed(format!("Failed to read display state: {error}")),
        ),
    }
}

fn send_event(event_sender: &async_channel::Sender<AppEvent>, event: AppEvent) {
    if event_sender.send_blocking(event).is_err() {
        log::debug!("UI event channel closed");
    }
}

fn install_event_handler(ui: Rc<Ui>, event_receiver: async_channel::Receiver<AppEvent>) {
    glib::spawn_future_local(async move {
        while let Ok(event) = event_receiver.recv().await {
            handle_event(&ui, event);
        }
        ui.window.destroy();
    });
}

fn handle_event(ui: &Ui, event: AppEvent) {
    match event {
        AppEvent::State(state) => {
            ui.state.replace(AppState::from_system(*state));
            rebuild(ui);
        }
        AppEvent::Applied(report) => {
            if let Some(warning) = report.warnings.first() {
                let toast = adw::Toast::new(warning);
                toast.set_timeout(8);
                ui.toast_overlay.add_toast(toast);
            } else {
                ui.toast_overlay.add_toast(adw::Toast::new(&format!(
                    "Applied profile: {}",
                    report.profile.label()
                )));
            }
        }
        AppEvent::Failed(message) => {
            ui.toast_overlay.add_toast(adw::Toast::new(&message));
        }
        AppEvent::Notice(message) => {
            ui.toast_overlay.add_toast(adw::Toast::new(&message));
        }
        AppEvent::Busy(busy) => {
            ui.state.borrow_mut().busy = busy;
            ui.spinner.set_visible(busy);
            ui.refresh_button.set_sensitive(!busy);
            rebuild(ui);
        }
        AppEvent::ShutdownComplete => {
            ui.window.destroy();
        }
    }
}

fn rebuild(ui: &Ui) {
    let state = ui.state.borrow();
    ui.status_row
        .set_subtitle(&view_model::status_summary(&state));
    ui.mirror_group.set_visible(state.mirror_running);

    clear_group(&ui.profiles_group, &ui.profile_widgets);
    for row in view_model::profile_rows(&state) {
        let widget = build_profile_row(&ui.actions, &row);
        ui.profiles_group.add(&widget);
        ui.profile_widgets.borrow_mut().push(widget.upcast());
    }

    clear_group(&ui.outputs_group, &ui.output_widgets);
    let outputs = view_model::output_rows(&state);
    if outputs.is_empty() {
        let empty = adw::ActionRow::builder()
            .title("No outputs detected")
            .subtitle("Check that niri is running in this session")
            .build();
        empty.set_activatable(false);
        ui.outputs_group.add(&empty);
        ui.output_widgets.borrow_mut().push(empty.upcast());
    } else {
        for row in outputs {
            let widget = build_info_row(&row);
            ui.outputs_group.add(&widget);
            ui.output_widgets.borrow_mut().push(widget.upcast());
        }
    }

    clear_group(&ui.ports_group, &ui.port_widgets);
    let ports = view_model::connector_rows(&state);
    if ports.is_empty() {
        let empty = adw::ActionRow::builder()
            .title("No DRM connectors found")
            .subtitle("The DRM sysfs interface is not available")
            .build();
        empty.set_activatable(false);
        ui.ports_group.add(&empty);
        ui.port_widgets.borrow_mut().push(empty.upcast());
    } else {
        for row in ports {
            let widget = build_info_row(&row);
            ui.ports_group.add(&widget);
            ui.port_widgets.borrow_mut().push(widget.upcast());
        }
    }
}

fn build_profile_row(
    actions: &async_channel::Sender<AppAction>,
    row: &ProfileRow,
) -> adw::ActionRow {
    let widget = adw::ActionRow::builder()
        .title(&row.title)
        .subtitle(&row.subtitle)
        .build();
    widget.set_activatable(true);
    widget.set_sensitive(row.sensitive);
    widget.add_prefix(&gtk::Image::from_icon_name(&row.icon_name));
    if row.active {
        let marker = gtk::Image::from_icon_name("object-select-symbolic");
        marker.set_tooltip_text(Some("This profile is active"));
        widget.add_suffix(&marker);
    }
    let actions = actions.clone();
    let profile = row.profile;
    widget.connect_activated(move |_| {
        let _ = actions.try_send(AppAction::ApplyProfile(profile));
    });
    widget
}

fn build_info_row(row: &InfoRow) -> adw::ActionRow {
    let widget = adw::ActionRow::builder()
        .title(&row.title)
        .subtitle(&row.subtitle)
        .build();
    widget.set_activatable(false);
    widget.add_prefix(&gtk::Image::from_icon_name(&row.icon_name));
    widget
}

fn clear_group(group: &adw::PreferencesGroup, widgets: &RefCell<Vec<gtk::Widget>>) {
    for widget in widgets.borrow_mut().drain(..) {
        group.remove(&widget);
    }
}
