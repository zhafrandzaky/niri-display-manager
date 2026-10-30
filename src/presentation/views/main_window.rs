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
use crate::cli::ParsedArgs;
use crate::domain::display::{DisplayOutput, OutputId};
use crate::domain::output::{OutputUpdate, Transform, VrrMode};
use crate::domain::profile::ProfileKind;
use crate::infrastructure::drm_sysfs::SysfsProbe;
use crate::infrastructure::niri_ipc::NiriCliClient;
use crate::infrastructure::process_runner::{MirrorPidFile, SystemCommandRunner, SystemSupervisor};
use crate::presentation::view_model::{self, AppAction, AppEvent, AppState, InfoRow, ProfileRow};
use crate::service::backup_service::{ConfigMode, ConfigStore, FileConfigStore};
use crate::service::config_paths::{Environment, resolve_paths};
use crate::service::display_service::DisplayService;

type Service = DisplayService<NiriCliClient, SysfsProbe, FileConfigStore, SystemSupervisor>;

/// Run the GTK application until the window closes.
///
/// Configuration paths are resolved before the GUI starts so `--print-paths`
/// can report them without opening a window.
pub fn run(args: ParsedArgs) -> anyhow::Result<glib::ExitCode> {
    let environment = Environment::from_process();
    let resolved = resolve_paths(&args.options, &environment)?;
    let store = FileConfigStore::discover(resolved, &environment)?;

    if args.options.print_paths {
        print!("{}", describe_paths(&store));
        return Ok(glib::ExitCode::SUCCESS);
    }

    let app = adw::Application::builder().application_id(APP_ID).build();
    let activated = Rc::new(std::cell::Cell::new(false));
    app.connect_activate(move |app| {
        // Secondary launches must not build a second window or worker.
        if activated.get() {
            return;
        }
        activated.set(true);
        build_ui(app, store.clone());
    });
    Ok(app.run_with_args(&args.forwarded))
}

fn describe_paths(store: &FileConfigStore) -> String {
    let mode = match store.mode() {
        ConfigMode::Modular => "modular (display file included by the main configuration)",
        ConfigMode::Inline => "inline (managed section in the main configuration)",
    };
    format!(
        "mode:          {mode}\nmain config:   {}\nmanaged file:  {}\nbackup:        {}\n",
        store.main_config_path().display(),
        store.managed_config_path().display(),
        store.backup_path().display(),
    )
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

fn build_ui(app: &adw::Application, store: FileConfigStore) {
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

    spawn_worker(action_receiver, event_sender, store);
    install_event_handler(ui, event_receiver);
}

fn spawn_worker(
    action_receiver: async_channel::Receiver<AppAction>,
    event_sender: async_channel::Sender<AppEvent>,
    store: FileConfigStore,
) {
    let service = build_service(store);
    let spawn_result = std::thread::Builder::new()
        .name("niri-display-manager-worker".to_owned())
        .spawn(move || worker_loop(service, action_receiver, event_sender));
    if let Err(error) = spawn_result {
        log::error!("failed to spawn worker thread: {error}");
    }
}

fn build_service(store: FileConfigStore) -> Service {
    DisplayService::new(
        NiriCliClient::new(std::sync::Arc::new(SystemCommandRunner)),
        SysfsProbe::system(),
        store,
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
            AppAction::ApplyOutput { output, update } => {
                send_event(&event_sender, AppEvent::Busy(true));
                match service.apply_output_config(&OutputId::new(output), &update) {
                    Ok(report) => send_event(&event_sender, AppEvent::Applied(Box::new(report))),
                    Err(error) => send_event(
                        &event_sender,
                        AppEvent::Failed(format!("Failed to apply output settings: {error}")),
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
            } else if report.profile == ProfileKind::Custom {
                ui.toast_overlay
                    .add_toast(adw::Toast::new("Applied output settings"));
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
    if state.outputs.is_empty() {
        let empty = adw::ActionRow::builder()
            .title("No outputs detected")
            .subtitle("Check that niri is running in this session")
            .build();
        empty.set_activatable(false);
        ui.outputs_group.add(&empty);
        ui.output_widgets.borrow_mut().push(empty.upcast());
    } else {
        for output in &state.outputs {
            let widget = build_output_expander(&ui.actions, &state, output);
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

fn build_output_expander(
    actions: &async_channel::Sender<AppAction>,
    state: &AppState,
    output: &DisplayOutput,
) -> adw::ExpanderRow {
    let expander = adw::ExpanderRow::builder()
        .title(&output.name)
        .subtitle(view_model::output_summary(output))
        .build();
    expander.add_prefix(&gtk::Image::from_icon_name(if output.is_internal() {
        "computer-symbolic"
    } else {
        "video-display-symbolic"
    }));

    let mode_labels = view_model::mode_labels(output);
    let mode_refs: Vec<&str> = mode_labels.iter().map(String::as_str).collect();
    let mode_model = gtk::StringList::new(&mode_refs);
    let mode_row = adw::ComboRow::builder()
        .title("Mode")
        .model(&mode_model)
        .build();
    mode_row.set_selected(view_model::selected_mode_index(output));
    mode_row.set_sensitive(!output.modes.is_empty());
    expander.add_row(&mode_row);

    let adjustment =
        gtk::Adjustment::new(view_model::initial_scale(output), 0.5, 3.0, 0.05, 0.1, 0.0);
    let scale_row = adw::SpinRow::builder()
        .title("Scale")
        .adjustment(&adjustment)
        .climb_rate(0.05)
        .digits(2)
        .build();
    expander.add_row(&scale_row);

    let transform_labels = view_model::transform_labels();
    let transform_model = gtk::StringList::new(&transform_labels);
    let transform_row = adw::ComboRow::builder()
        .title("Transform")
        .model(&transform_model)
        .build();
    transform_row.set_selected(view_model::initial_transform_index(output));
    expander.add_row(&transform_row);

    let vrr_labels = view_model::vrr_labels();
    let vrr_model = gtk::StringList::new(&vrr_labels);
    let vrr_row = adw::ComboRow::builder()
        .title("Variable refresh rate")
        .model(&vrr_model)
        .build();
    let stored = state.output_preferences.get(&output.name);
    vrr_row.set_selected(view_model::initial_vrr_index(output, stored));
    vrr_row.set_sensitive(output.vrr_supported);
    expander.add_row(&vrr_row);

    let enabled_row = adw::SwitchRow::builder()
        .title("Enabled")
        .active(output.is_enabled())
        .build();
    if output.is_enabled() && !view_model::can_disable_output(state, &output.name) {
        enabled_row.set_sensitive(false);
        enabled_row.set_subtitle("At least one display must remain enabled");
    }
    expander.add_row(&enabled_row);

    let apply_row = adw::ActionRow::builder()
        .title("Apply output settings")
        .build();
    apply_row.set_activatable(false);
    let apply_button = gtk::Button::builder().label("Apply").build();
    apply_button.add_css_class("suggested-action");
    apply_row.add_suffix(&apply_button);
    expander.add_row(&apply_row);

    let modes = output.modes.clone();
    let position = output
        .logical
        .as_ref()
        .map(|logical| (logical.x, logical.y));
    let output_name = output.name.clone();
    let actions = actions.clone();
    apply_button.connect_clicked(move |_| {
        let update = OutputUpdate {
            enabled: enabled_row.is_active(),
            mode: view_model::mode_at(&modes, mode_row.selected()),
            scale: Some(scale_row.value()),
            transform: Some(Transform::from_index(transform_row.selected())),
            position,
            vrr: Some(VrrMode::from_index(vrr_row.selected())),
        };
        let _ = actions.try_send(AppAction::ApplyOutput {
            output: output_name.clone(),
            update,
        });
    });

    if state.busy {
        expander.set_sensitive(false);
    }
    expander
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
