#![cfg(target_os = "windows")]

use log::{debug, error, info};
use once_cell::sync::Lazy;
use sdl3::gamepad;
use sdl3::event::Event;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::thread;
use vigem_client::{Client, XButtons, XGamepad};

enum AppState {
    DETECT_CONFIG,
    ACCEPTING_INPUT,
}

#[derive(PartialEq)]
enum VigemInput {
    Button(u16),
    LeftTrigger,
    RightTrigger,
}

struct MashingTrigger {
    button: VigemInput,
    is_pressed: bool,
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
    let mut current_controller = gamepad_system
        .gamepads()
        .unwrap()
        .into_iter()
        .find_map(|j| match gamepad_system.open(j) {
            Ok(c) => {
                println!("Success: opened controller \"{}\"", c.name().unwrap());
                Some(c)
            }
            Err(e) => {
                println!("failed: {:?}", e);
                None
            }
        })
        .expect("failed to open any joysticks");

    let client = vigem_client::Client::connect().unwrap();
    let id = vigem_client::TargetId::XBOX360_WIRED;
    let mut target = vigem_client::Xbox360Wired::new(client, id);
    target.plugin().map_err(|e| e.to_string());
    target.wait_ready().map_err(|e| e.to_string());
    let mut gamepad_state = vigem_client::XGamepad::default();

    let current_app_state = AppState::ACCEPTING_INPUT;
    let mut mashing_trigger_state: Vec<MashingTrigger> = Vec::new();
    let mut should_mash = false;
    let mut config_held_buttons: HashMap<u32, Vec<VigemInput>> = HashMap::new();

    let mut event_pump = sdl_context.event_pump().unwrap();
    'mainloop: loop {
        event_pump.pump_events();
        for event in event_pump.poll_iter() {
            match current_app_state {
                AppState::ACCEPTING_INPUT => {
                    match event {
                        Event::ControllerButtonDown { which, button, .. } => {
                            println!("controller down {}", button.string());
                            if which != current_controller.id().unwrap_or(u32::MAX) { continue; }

                            if let Some(vigem_button) = sdl_button_to_vigem(button) {
                                if let Some(pressed_button) = mashing_trigger_state.iter_mut().find(|trigger| trigger.button == vigem_button) {
                                    pressed_button.is_pressed = true;
                                }
                            }
                        }
                        Event::ControllerButtonUp { which, button, .. } => {
                            println!("controller up {}", button.string());
                            if which != current_controller.id().unwrap_or(u32::MAX) { continue; }

                            if let Some(vigem_button) = sdl_button_to_vigem(button) {
                                if let Some(pressed_button) = mashing_trigger_state.iter_mut().find(|trigger| trigger.button == vigem_button) {
                                    pressed_button.is_pressed = false;
                                }
                            }
                        }

                        Event::ControllerAxisMotion { which, axis, value, .. } => {
                            match axis {
                                sdl3::gamepad::Axis::TriggerLeft => {
                                    if which != current_controller.id().unwrap_or(u32::MAX) { continue; }

                                    if let Some(pressed_trigger) = mashing_trigger_state.iter_mut().find(|m| m.button == VigemInput::LeftTrigger) {
                                        pressed_trigger.is_pressed = value != 0;
                                    }
                                }
                                sdl3::gamepad::Axis::TriggerRight => {
                                    if which != current_controller.id().unwrap_or(u32::MAX) { continue; }

                                    if let Some(pressed_trigger) = mashing_trigger_state.iter_mut().find(|m| m.button == VigemInput::RightTrigger) {
                                        pressed_trigger.is_pressed = value != 0;
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
                    should_mash = mashing_trigger_state.iter().all(|t| t.is_pressed);
                }

                AppState::DETECT_CONFIG => {}
            }
        }
    }
}
