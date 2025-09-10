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
}

#[derive(Serialize, Deserialize)]
struct Settings {
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

    let mut current_app_state = AppState::AcceptingInput;
    // Read from settings file
    let mut settings_path = base_path.clone();
    settings_path.push("HKCM_settings.json");

    let mut settings: Settings = if !settings_path.exists() {
        let default_config = Settings{mashing_triggers: vec![VigemInput::Button(1), VigemInput::LeftTrigger, VigemInput::Button(32)]};
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

    // App state setup
    sdl3::hint::set("SDL_HINT_JOYSTICK_ALLOW_BACKGROUND_EVENTS", "1");
    let sdl_context = sdl3::init().unwrap();
    let gamepad_system = sdl_context.gamepad().unwrap();

    let mut held_buttons: HashMap<u32, Vec<VigemInput>> = HashMap::new();
    // we need a reference to an open gamepad for it to stay open
    let mut _opened_gamepads: HashMap<u32, sdl3::gamepad::Gamepad> = HashMap::new();

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
        let mut should_mash = false;

        event_pump.pump_events();
        for event in event_pump.poll_iter() {
            match event {
                Event::ControllerDeviceAdded { which, .. } => {
                    if let Ok(gamepad) = gamepad_system.open(which) {
                        _opened_gamepads.insert(which, gamepad);
                    }
                }
                Event::ControllerDeviceRemoved { which, .. }         => {
                    _opened_gamepads.remove(&which);
                }
                Event::ControllerButtonDown { which, button, .. } => {
                    debug!("controller down {}", button.string());
                    
                    if let Some(input) = sdl_button_to_vigem(button) {
                        if !held_buttons.contains_key(&which) {
                            held_buttons.insert(which, vec![input]);
                        } else {
                            if let Some(held) = held_buttons.get_mut(&which) {
                                if !held.iter().any(|x| *x == input) {
                                    held.push(input);
                                }
                            }
                        }
                    }
                    // if button == sdl3::gamepad::Button::Start {
                    //     debug!("ENTERING CONFIG DETECTION MODE\n============================");
                    //     current_app_state = AppState::DetectConfig;
                    //     continue 'mainloop;
                    // }
                }
                Event::ControllerButtonUp { which, button, .. } => {
                    debug!("controller up {}", button.string());

                    if let Some(entry) = held_buttons.get_mut(&which) {
                        if let Some(input) = sdl_button_to_vigem(button) {
                            entry.retain(|held| *held != input);

                            if entry.is_empty() {
                                held_buttons.remove_entry(&which);
                            }
                        }
                    }
                }
                Event::ControllerAxisMotion { which, axis, value, .. } => {
                    let converted_input = match axis {
                        gamepad::Axis::TriggerLeft => Some(VigemInput::LeftTrigger),
                        gamepad::Axis::TriggerRight => Some(VigemInput::RightTrigger),
                        _ => None
                    };

                    if let Some(input) = converted_input {
                        if value > 0 {
                            if !held_buttons.contains_key(&which) {
                                held_buttons.insert(which, vec![input]);
                            } else {
                                if let Some(held) = held_buttons.get_mut(&which) {
                                    if !held.iter().any(|x| *x == input) {
                                        held.push(input);
                                    }
                                }
                            }
                        } 
                        else {
                            if let Some(entry) = held_buttons.get_mut(&which) {
                                entry.retain(|held| *held != input);

                                if entry.is_empty() {
                                    held_buttons.remove_entry(&which);
                                }
                            }
                        }
                    }
                }

                Event::Quit { .. } => break 'mainloop,
                _ => (),
            }

            // the mashing controller will never be holding all 3 so there
            // isnt risk of a feedback loop
            // config just needs to hold the mashing keys, and any controller
            // can press them to activate the masher
            if matches!(current_app_state, AppState::AcceptingInput) {
                // dbg!(&held_buttons);
                for (_, val) in held_buttons.iter() {
                    if val.len() >= MAX_MASHING_KEY_COUNT as usize {
                        // check if all triggers are pressed and activate the mashing
                        should_mash = mashing_buttons.read().unwrap().iter().all(|button| val.contains(button));
                    }
                }

                if IS_MASHER_ACTIVE.load(Ordering::SeqCst) != should_mash {
                    debug!("all mashing triggers pressed: {}", should_mash);
                    IS_MASHER_ACTIVE.store(should_mash, Ordering::SeqCst);
                }
            }
            else if matches!(current_app_state, AppState::DetectConfig) {
                for (_, val) in held_buttons.iter() {
                    if val.len() == MAX_MASHING_KEY_COUNT as usize {
                        {
                            *mashing_buttons.write().expect("Failed to get state while storing config") = val.clone();
                        }

                        current_app_state = AppState::AcceptingInput;

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

}
