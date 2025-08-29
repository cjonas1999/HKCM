#![cfg(target_os = "windows")]


mod livesplit_core;
mod text_masher;

use log::{debug, error, info};
use once_cell::sync::Lazy;
use sdl3::gamepad;
use sdl3::event::Event;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::thread;
use vigem_client::{Client, XButtons, XGamepad};
use crate::text_masher::{text_masher, IS_MASHER_ACTIVE, MAX_MASHING_KEY_COUNT};

enum AppState {
    DetectConfig,
    AcceptingInput,
}

#[derive(PartialEq, Debug, Clone)]
enum VigemInput {
    Button(u16),
    LeftTrigger,
    RightTrigger,
}

fn sdl_button_to_vigem(button: sdl3::gamepad::Button) -> Option<VigemInput> {
    match button {
        sdl3::gamepad::Button::North => Some(VigemInput::Button(vigem_client::XButtons::Y)),
        sdl3::gamepad::Button::East => Some(VigemInput::Button(vigem_client::XButtons::B)),
        sdl3::gamepad::Button::South => Some(VigemInput::Button(vigem_client::XButtons::A)),
        sdl3::gamepad::Button::West => Some(VigemInput::Button(vigem_client::XButtons::X)),
        sdl3::gamepad::Button::Back => Some(VigemInput::Button(vigem_client::XButtons::BACK)),
        sdl3::gamepad::Button::Guide => Some(VigemInput::Button(vigem_client::XButtons::GUIDE)),
        sdl3::gamepad::Button::Start => Some(VigemInput::Button(vigem_client::XButtons::START)),
        sdl3::gamepad::Button::LeftStick => Some(VigemInput::Button(vigem_client::XButtons::LTHUMB)),
        sdl3::gamepad::Button::RightStick => Some(VigemInput::Button(vigem_client::XButtons::RTHUMB)),
        sdl3::gamepad::Button::LeftShoulder => Some(VigemInput::Button(vigem_client::XButtons::LB)),
        sdl3::gamepad::Button::RightShoulder => Some(VigemInput::Button(vigem_client::XButtons::RB)),
        sdl3::gamepad::Button::DPadUp => Some(VigemInput::Button(vigem_client::XButtons::UP)),
        sdl3::gamepad::Button::DPadDown => Some(VigemInput::Button(vigem_client::XButtons::DOWN)),
        sdl3::gamepad::Button::DPadLeft => Some(VigemInput::Button(vigem_client::XButtons::LEFT)),
        sdl3::gamepad::Button::DPadRight => Some(VigemInput::Button(vigem_client::XButtons::RIGHT)),
        _ => None, // not supported in vigem
    }
}

fn main() {

    sdl3::hint::set("SDL_HINT_JOYSTICK_ALLOW_BACKGROUND_EVENTS", "1");
    let sdl_context = sdl3::init().unwrap();
    let gamepad_system = sdl_context.gamepad().unwrap();
    // TODO: find controller by id from config file
    // need to be able to update this when configuring
    let mut _opened_gamepads: Vec<sdl3::gamepad::Gamepad> = Vec::new();
    let mut current_controller = gamepad_system
        .gamepads()
        .unwrap()
        .into_iter()
        .map(|j| match gamepad_system.open(j) {
            Ok(c) => {
                println!("Success: opened controller \"{}\"", c.name().unwrap());
                Some(c)
            }
            Err(e) => {
                println!("failed: {:?}", e);
                None
            }
        })
        .flatten()
        .collect::<Vec<sdl3::gamepad::Gamepad>>()
        .remove(0);


    // App state setup
    let mut current_app_state = AppState::AcceptingInput;
    let mut should_mash = false;
    let mut config_held_buttons: HashMap<u32, Vec<VigemInput>> = HashMap::new();
    let mut config_detect_needs_initialized = true;
    let mut buttons_held: Vec<bool> = vec![false; 3];

    let mashing_buttons: Arc<RwLock<Vec<VigemInput>>> = Arc::new(std::sync::RwLock::new(Vec::new()));
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
                            gamepad_state.buttons = vigem_client::XButtons(*b)
                        }
                        VigemInput::LeftTrigger => gamepad_state.left_trigger = u8::MAX,
                        VigemInput::RightTrigger => gamepad_state.right_trigger = u8::MAX,
                    }
                }
            }

            target.update(&gamepad_state).expect("Failed to update virtual controller while mashing");
        });
    });

    let mut event_pump = sdl_context.event_pump().unwrap();
    'mainloop: loop {
        //===============================
        // Accepting Input for Mashing
        //===============================
        if matches!(current_app_state, AppState::AcceptingInput) {
            event_pump.pump_events();
            for event in event_pump.poll_iter() {
                match event {
                    Event::ControllerButtonDown { which, button, .. } => {
                        println!("controller down {}", button.string());
                        if which != current_controller.id().unwrap_or(u32::MAX) { continue; }

                        if let Some(vigem_button) = sdl_button_to_vigem(button) {
                            if let Some(index) = mashing_buttons.read().unwrap().iter().position(|trigger| *trigger == vigem_button) {
                                buttons_held[index] = true;
                            }
                        }

                        if button == sdl3::gamepad::Button::Start {
                            println!("ENTERING CONFIG DETECTION MODE\n============================");
                            current_app_state = AppState::DetectConfig;
                            continue 'mainloop;
                        }
                    }
                    Event::ControllerButtonUp { which, button, .. } => {
                        println!("controller up {}", button.string());
                        if which != current_controller.id().unwrap_or(u32::MAX) { continue; }

                        if let Some(vigem_button) = sdl_button_to_vigem(button) {
                            if let Some(index) = mashing_buttons.read().unwrap().iter().position(|trigger| *trigger == vigem_button) {
                                buttons_held[index] = false;
                            }
                        }
                    }

                    Event::ControllerAxisMotion { which, axis, value, .. } => {
                        match axis {
                            sdl3::gamepad::Axis::TriggerLeft => {
                                if which != current_controller.id().unwrap_or(u32::MAX) { continue; }

                                if let Some(index) = mashing_buttons.read().unwrap().iter().position(|m| *m == VigemInput::LeftTrigger) {
                                    buttons_held[index] = value != 0;
                                }
                            }
                            sdl3::gamepad::Axis::TriggerRight => {
                                if which != current_controller.id().unwrap_or(u32::MAX) { continue; }

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
                should_mash = buttons_held.iter().all(|t| *t);
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
                            println!("Success: opened controller \"{}\"", c.name().unwrap());
                            Some(c)
                        }
                        Err(e) => {
                            println!("failed: {:?}", e);
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
                        println!("controller down {}", button.string());
                        
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

                        dbg!(&config_held_buttons);
                    }
                    Event::ControllerButtonUp { which, button, .. } => {
                        println!("controller up {}", button.string());

                        if let Some(entry) = config_held_buttons.get_mut(&which) {
                            if let Some(input) = sdl_button_to_vigem(button) {
                                entry.retain(|held| *held != input);

                                if entry.is_empty() {
                                    config_held_buttons.remove_entry(&which);
                                }
                            }
                        }
                        dbg!(&config_held_buttons);
                    }

                    Event::ControllerAxisMotion { which, axis, value, .. } => {
                        match axis {
                            sdl3::gamepad::Axis::TriggerLeft => {
                                println!("left trigger {}", value);
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
                                println!("right trigger {}", value);
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
                            current_controller = gamepad_system.open(*k).unwrap();
                            {
                                *mashing_buttons.write().expect("Failed to get state while storing config") = val.clone();
                            }

                            current_app_state = AppState::AcceptingInput;
                            config_detect_needs_initialized = true;
                            println!("ENTERING ACCEPTING INPUT MODE\n===================================");

                            continue 'mainloop;
                        }
                    }
                }
            }
        }
    }
}
