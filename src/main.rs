#![cfg(target_os = "windows")]

mod livesplit_core;
mod text_masher;

use log::{debug, error, info};
use serde::{Serialize, Deserialize};
use std::fs::File;
use std::io::Write;
use humantime;
use std::time::SystemTime;
use sdl3::gamepad;
use sdl3::event::Event;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, RwLock};
use std::thread;
use vigem_client::XButtons;
use crate::text_masher::{text_masher, IS_MASHER_ACTIVE, MAX_MASHING_KEY_COUNT};

enum AppState {
    DetectConfig,
    AcceptingInput,
    DetectingController,
}

#[derive(Serialize, Deserialize)]
struct Settings {
    controller_guid: String,
    mashing_triggers: Vec<VigemInput>,
}

#[derive(PartialEq, Debug, Clone, Serialize, Deserialize)]
enum VigemInput {
    Button(u16),
    LeftTrigger,
    RightTrigger,
}

fn sdl_button_to_vigem(button: gamepad::Button) -> Option<VigemInput> {
    match button {
        gamepad::Button::North => Some(VigemInput::Button(XButtons::Y)),
        gamepad::Button::East => Some(VigemInput::Button(XButtons::B)),
        gamepad::Button::South => Some(VigemInput::Button(XButtons::A)),
        gamepad::Button::West => Some(VigemInput::Button(XButtons::X)),
        gamepad::Button::Back => Some(VigemInput::Button(XButtons::BACK)),
        gamepad::Button::Guide => Some(VigemInput::Button(XButtons::GUIDE)),
        gamepad::Button::Start => Some(VigemInput::Button(XButtons::START)),
        gamepad::Button::LeftStick => Some(VigemInput::Button(XButtons::LTHUMB)),
        gamepad::Button::RightStick => Some(VigemInput::Button(XButtons::RTHUMB)),
        gamepad::Button::LeftShoulder => Some(VigemInput::Button(XButtons::LB)),
        gamepad::Button::RightShoulder => Some(VigemInput::Button(XButtons::RB)),
        gamepad::Button::DPadUp => Some(VigemInput::Button(XButtons::UP)),
        gamepad::Button::DPadDown => Some(VigemInput::Button(XButtons::DOWN)),
        gamepad::Button::DPadLeft => Some(VigemInput::Button(XButtons::LEFT)),
        gamepad::Button::DPadRight => Some(VigemInput::Button(XButtons::RIGHT)),
        _ => None, // not supported in vigem
    }
}

fn main() {
    let mut base_path = dirs::data_dir().unwrap();
    base_path.push("HKCM");
    std::fs::create_dir_all(&base_path).unwrap();

    let mut log_file_path = base_path.clone();
    log_file_path.push("HKCM_log.txt");
    let log_file = fern::log_file(log_file_path).unwrap();
    // Configure Logger
    fern::Dispatch::new()
        .format(|out, message, record| {
            out.finish(format_args!(
                "[{} {} {}] {}",
                humantime::format_rfc3339_seconds(SystemTime::now()),
                record.level(),
                record.target(),
                message
            ))
        })
        .level(log::LevelFilter::Debug)
        .chain(std::io::stdout())
        .chain(log_file)
        .apply()
        .expect("Failed to configure logging");

    let mut current_app_state = AppState::DetectingController;
    // Read from settings file
    let mut settings_path = base_path.clone();
    settings_path.push("HKCM_settings.json");

    let mut settings: Settings = if !settings_path.exists() {
        let default_config = Settings{controller_guid: String::from(""), mashing_triggers: vec![VigemInput::Button(1), VigemInput::LeftTrigger, VigemInput::Button(32)]};
        let json = serde_json::to_string_pretty(&default_config).expect("Failed to convert config to json");
        let mut file = File::create(&settings_path).unwrap();
        file.write_all(json.as_bytes()).expect("Failed to write config to file");

        default_config
    }
    else {
        let file = File::open(&settings_path).unwrap();

        // TODO: handle malformed file
        serde_json::from_reader(file).expect("Could not parse settings file")
    };

    info!("{}", settings.controller_guid);

    // App state setup
    sdl3::hint::set("SDL_HINT_JOYSTICK_ALLOW_BACKGROUND_EVENTS", "1");
    let sdl_context = sdl3::init().unwrap();
    let gamepad_system = sdl_context.gamepad().unwrap();

    let mut should_mash = false;
    let mut config_held_buttons: HashMap<u32, Vec<VigemInput>> = HashMap::new();
    let mut config_detect_needs_initialized = true;
    let mut buttons_held: Vec<bool> = vec![false; 3];
    // we need a reference to an open gamepad for it to stay open
    let mut _opened_gamepads: Vec<sdl3::gamepad::Gamepad> = Vec::new();

    let mut current_controller: Option<sdl3::gamepad::Gamepad> = None;

    let mashing_buttons: Arc<RwLock<Vec<VigemInput>>> = Arc::new(std::sync::RwLock::new(settings.mashing_triggers.clone()));
    let thread_mashing_buttons = Arc::clone(&mashing_buttons);

    thread::spawn(move || {
        // VIGEM setup
        let client = vigem_client::Client::connect().unwrap();
        let id = vigem_client::TargetId::XBOX360_WIRED;
        let mut target = vigem_client::Xbox360Wired::new(client, id);
        target.plugin().expect("Failed to plugin virtual controller");
        target.wait_ready().expect("Could not wait for virtual controller to ready");

        text_masher(|key_to_press| {
            let mut gamepad_state = vigem_client::XGamepad::default();

            if key_to_press < MAX_MASHING_KEY_COUNT {
                let mash_buttons = thread_mashing_buttons.read().unwrap();
                if let Some(press) = mash_buttons.get(key_to_press as usize) {
                    match press {
                        VigemInput::Button(b) => {
                            gamepad_state.buttons = XButtons(*b)
                        }
                        VigemInput::LeftTrigger => gamepad_state.left_trigger = u8::MAX,
                        VigemInput::RightTrigger => gamepad_state.right_trigger = u8::MAX,
                    }
                }
            }

            target.update(&gamepad_state).expect("Failed to update virtual controller while mashing");
        });
    });

    info!("Initialization complete");
    let mut event_pump = sdl_context.event_pump().unwrap();
    'mainloop: loop {
        //===============================
        // Accepting Input for Mashing
        //===============================
        if matches!(current_app_state, AppState::AcceptingInput) {
            if current_controller.as_ref().is_none() {
                current_app_state = AppState::DetectingController;
                continue 'mainloop;
            }
            event_pump.pump_events();
            for event in event_pump.poll_iter() {
                match event {
                    Event::ControllerButtonDown { which, button, .. } => {
                        debug!("controller down {}", button.string());
                        if which != current_controller.as_ref().unwrap().id().unwrap_or(u32::MAX) { continue; }

                        if let Some(vigem_button) = sdl_button_to_vigem(button) {
                            if let Some(index) = mashing_buttons.read().unwrap().iter().position(|trigger| *trigger == vigem_button) {
                                buttons_held[index] = true;
                            }
                        }

                        if button == sdl3::gamepad::Button::Start {
                            debug!("ENTERING CONFIG DETECTION MODE\n============================");
                            current_app_state = AppState::DetectConfig;
                            continue 'mainloop;
                        }
                    }
                    Event::ControllerButtonUp { which, button, .. } => {
                        debug!("controller up {}", button.string());
                        if which != current_controller.as_ref().unwrap().id().unwrap_or(u32::MAX) { continue; }

                        if let Some(vigem_button) = sdl_button_to_vigem(button) {
                            if let Some(index) = mashing_buttons.read().unwrap().iter().position(|trigger| *trigger == vigem_button) {
                                buttons_held[index] = false;
                            }
                        }
                    }

                    Event::ControllerAxisMotion { which, axis, value, .. } => {
                        match axis {
                            sdl3::gamepad::Axis::TriggerLeft => {
                                if which != current_controller.as_ref().unwrap().id().unwrap_or(u32::MAX) { continue; }

                                if let Some(index) = mashing_buttons.read().unwrap().iter().position(|m| *m == VigemInput::LeftTrigger) {
                                    buttons_held[index] = value != 0;
                                }
                            }
                            sdl3::gamepad::Axis::TriggerRight => {
                                if which != current_controller.as_ref().unwrap().id().unwrap_or(u32::MAX) { continue; }

                                if let Some(index) = mashing_buttons.read().unwrap().iter().position(|m| *m == VigemInput::RightTrigger) {
                                    buttons_held[index] = value != 0;
                                }
                            }
                            _ => (),
                        }
                    },

                    Event::KeyDown { keycode: Some(sdl3::keyboard::Keycode::Escape), .. } => break 'mainloop,
                    Event::Quit { .. } => break 'mainloop,
                    _ => (),
                }

                // check if all triggers are pressed and activate the mashing
                should_mash = buttons_held.len() > 0 && buttons_held.iter().all(|t| *t);
            }

            if IS_MASHER_ACTIVE.load(Ordering::SeqCst) != should_mash {
                debug!("all mashing triggers pressed: {}", should_mash);
                IS_MASHER_ACTIVE.store(should_mash, Ordering::SeqCst);
            }
        }


        //========================
        // Config Detection
        // =======================
        else if matches!(current_app_state, AppState::DetectConfig) {
            let mut config_finalized = false;

            if config_detect_needs_initialized {
                config_held_buttons = HashMap::new();
                config_detect_needs_initialized = false;

                // open all gamepads
                _opened_gamepads = gamepad_system
                    .gamepads()
                    .unwrap()
                    .into_iter()
                    .map(|j| match gamepad_system.open(j) {
                        Ok(c) => {
                            info!("Success: opened controller \"{}\"", c.name().unwrap());
                            Some(c)
                        }
                        Err(e) => {
                            error!("failed: {:?}", e);
                            None
                        }
                    })
                    .flatten()
                    .collect();
            }

            
            event_pump.pump_events();
            for event in event_pump.poll_iter() {
                match event {
                    Event::ControllerButtonDown { which, button, .. } => {
                        debug!("controller down {}", button.string());
                        
                        if let Some(input) = sdl_button_to_vigem(button) {
                            if !config_held_buttons.contains_key(&which) {
                                config_held_buttons.insert(which, vec![input]);
                            } else {

                                if let Some(held) = config_held_buttons.get_mut(&which) {
                                    held.push(input);

                                    if held.len() == MAX_MASHING_KEY_COUNT as usize {
                                        config_finalized = true;
                                    }
                                }
                            }
                        }
                    }
                    Event::ControllerButtonUp { which, button, .. } => {
                        debug!("controller up {}", button.string());

                        if let Some(entry) = config_held_buttons.get_mut(&which) {
                            if let Some(input) = sdl_button_to_vigem(button) {
                                entry.retain(|held| *held != input);

                                if entry.is_empty() {
                                    config_held_buttons.remove_entry(&which);
                                }
                            }
                        }
                    }

                    Event::ControllerAxisMotion { which, axis, value, .. } => {
                        match axis {
                            sdl3::gamepad::Axis::TriggerLeft => {
                                debug!("left trigger {}", value);
                                let input = VigemInput::LeftTrigger;
                            
                                if value > 0 {
                                    if !config_held_buttons.contains_key(&which) {
                                        config_held_buttons.insert(which, vec![input]);
                                    } else {
                                        if let Some(held) = config_held_buttons.get_mut(&which) {
                                            if !held.iter().any(|x| *x == input) {
                                                held.push(input);
                                            }

                                            if held.len() == MAX_MASHING_KEY_COUNT as usize {
                                                config_finalized = true;
                                            }
                                        }
                                    }
                                } else {
                                    if let Some(entry) = config_held_buttons.get_mut(&which) {
                                        entry.retain(|held| *held != input);

                                        if entry.is_empty() {
                                            config_held_buttons.remove_entry(&which);
                                        }
                                    }
                                }
                            }
                            sdl3::gamepad::Axis::TriggerRight => {
                                debug!("right trigger {}", value);
                                let input = VigemInput::RightTrigger;
                            
                                if value > 0 {
                                    if !config_held_buttons.contains_key(&which) {
                                        config_held_buttons.insert(which, vec![input]);
                                    } else {
                                        if let Some(held) = config_held_buttons.get_mut(&which) {
                                            if !held.iter().any(|x| *x == input) {
                                                held.push(input);
                                            }

                                            if held.len() == MAX_MASHING_KEY_COUNT as usize {
                                                config_finalized = true;
                                            }
                                        }
                                    }
                                } else {
                                    if let Some(entry) = config_held_buttons.get_mut(&which) {
                                        entry.retain(|held| *held != input);

                                        if entry.is_empty() {
                                            config_held_buttons.remove_entry(&which);
                                        }
                                    }
                                }
                            }
                            _ => (),
                        }
                    },

                    Event::KeyDown { keycode: Some(sdl3::keyboard::Keycode::Escape), .. } => break 'mainloop,
                    Event::Quit { .. } => break 'mainloop,
                    _ => (),
                }

                if config_finalized {
                    for (k, val) in config_held_buttons.iter() {
                        if val.len() == MAX_MASHING_KEY_COUNT as usize {
                            current_controller = gamepad_system.open(*k).ok();
                            {
                                *mashing_buttons.write().expect("Failed to get state while storing config") = val.clone();
                            }

                            current_app_state = AppState::AcceptingInput;
                            config_detect_needs_initialized = true;

                            if let Some(ref c) = current_controller {
                                settings.controller_guid = gamepad_system.guid_for_id(c.id().unwrap()).string();
                            }
                            else {
                                error!("Could not set controller id");
                            }
                            settings.mashing_triggers = val.clone();

                            let json = serde_json::to_string_pretty(&settings).expect("Failed to convert config to json");
                            let mut file = File::create(&settings_path).unwrap();
                            file.write_all(json.as_bytes()).expect("Failed to write config to file");
                            info!("ENTERING ACCEPTING INPUT MODE\n===================================");
                            continue 'mainloop;
                        }
                    }
                }
            }
        }


        
        //========================
        // Controller not connected
        // =======================
        else if matches!(current_app_state, AppState::DetectingController) {
            // TODO:init controller from id in settings file

            // TODO: handle when no controllers are plugged in
            // open will not work if controller isnt plugged in

            // Identify gamepad with correct id
            if let Some(pad_index) = gamepad_system.gamepads().unwrap().iter().find(|&joystick_id| {
                let joyname = gamepad_system.guid_for_id(*joystick_id).string();
                debug!("detected {} == {}, {}", joyname, settings.controller_guid, joyname == settings.controller_guid);
                joyname == settings.controller_guid
            }) {
                info!("in here");
                if let Ok(pad) = gamepad_system.open(*pad_index) {
                    info!("Success: opened controller \"{}\"", pad.name().unwrap());
                    current_controller = Some(pad);
                    current_app_state = AppState::AcceptingInput;
                    continue 'mainloop;
                } else {
                    error!("Could not open controller {:?}", settings.controller_guid);
                    current_controller = None;
                }
            }
        }
    }

}
