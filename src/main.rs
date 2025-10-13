#![cfg(target_os = "windows")]

mod livesplit_core;
mod text_masher;

use log::LevelFilter;
use log4rs::append::console::ConsoleAppender;
use log4rs::encode::pattern::PatternEncoder;
use log4rs::append::rolling_file::policy::compound::roll::delete::DeleteRoller;
use log4rs::append::rolling_file::policy::compound::trigger::size::SizeTrigger;
use log4rs::append::rolling_file::policy::compound::CompoundPolicy;
use log4rs::config::{Appender, Config, Root};
use log::{debug, error, info};
use sdl3::render::{Canvas, TextureCreator};
use serde::{Serialize, Deserialize};
use std::fs::File;
use std::io::Write;
use sdl3::gamepad;
use sdl3::event::Event;
use sdl3::rect::Rect;
use sdl3::pixels::Color;
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

#[derive(PartialEq, Debug, Clone, Serialize, Deserialize, Hash, Eq)]
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

struct InputDisplay {
    rect: Rect,
}

static INPUT_DEFAULT_COLOR: Color = Color::RGB(80, 80, 80);
static INPUT_HELD_COLOR: Color = Color::RGB(150, 150, 150);

impl InputDisplay {
    fn draw(&self, canvas: &mut sdl3::render::WindowCanvas, highlight: bool) {
        if highlight {
            canvas.set_draw_color(INPUT_HELD_COLOR);
        } else {
            canvas.set_draw_color(INPUT_DEFAULT_COLOR);
        }
        canvas.fill_rect(self.rect).expect("Failed rendering background");
    }
}


fn main() {
    let mut base_path = dirs::data_dir().unwrap();
    base_path.push("HKCM");
    std::fs::create_dir_all(&base_path).unwrap();

    let mut log_file_path = base_path.clone();
    log_file_path.push("HKCM_log.txt");

    // Configure Logger
    let log_pattern = "{d(%Y-%m-%d %H:%M:%S)} [{l}] {M}:{L} - {m}{n}";
    let console_log_appender = ConsoleAppender::builder()
        .encoder(Box::new(PatternEncoder::new(log_pattern)))
        .build();

    let log_file_appender = log4rs::append::rolling_file::RollingFileAppender::builder()
        .encoder(Box::new(PatternEncoder::new(log_pattern)))
        .build(log_file_path,
            Box::new(CompoundPolicy::new(
                Box::new(SizeTrigger::new(1024 * 1024 * 10)),
                Box::new(DeleteRoller::new()),
            )),
        ).unwrap();

    let config = Config::builder()
        .appender(Appender::builder().build("console", Box::new(console_log_appender)))
        .appender(Appender::builder().build("file", Box::new(log_file_appender)))
        .build(
            Root::builder()
                .appender("console")
                .appender("file")
                .build(LevelFilter::Debug),
            // TODO: dynamically select filter level in debug vs release builds
        ).unwrap();

    log4rs::init_config(config).unwrap();

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
    sdl3::hint::set("SDL_JOYSTICK_ALLOW_BACKGROUND_EVENTS", "1");
    let sdl_context = sdl3::init().unwrap();
    let gamepad_system = sdl_context.gamepad().unwrap();

    let mut held_buttons: HashMap<u32, Vec<VigemInput>> = HashMap::new();
    // we need a reference to an open gamepad for it to stay open
    let mut _opened_gamepads: HashMap<u32, sdl3::gamepad::Gamepad> = HashMap::new();

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

    // Initialize GUI
    let video_subsystem = sdl_context.video().unwrap();

    let window = video_subsystem.window("rust-sdl3 demo", 800, 600)
        .position_centered()
        .build()
        .unwrap();
    let mut canvas = window.into_canvas();
    let texture_creator = canvas.texture_creator();

    let ttf_context = sdl3::ttf::init().unwrap();
    const FONT_DATA: &[u8] = include_bytes!("../fonts/Roboto-Regular.ttf");
    let font_stream = sdl3::iostream::IOStream::from_bytes(FONT_DATA).expect("Failed to read font data");
    let font = ttf_context.load_font_from_iostream(font_stream, 50.0).unwrap();

    let surface = font
        .render("Configure")
        .blended(Color::RGBA(250, 250, 250, 255))
        .map_err(|e| e.to_string()).unwrap();
    let texture = texture_creator
        .create_texture_from_surface(&surface)
        .map_err(|e| e.to_string()).unwrap();

    let sdl3::render::TextureQuery { width, height, .. } = texture.query();

    // TODO: make configure button more responsive with changing color or text or something when in
    // detection mode. a way to cancel configuration mode, either by pressing again or adding
    // another button would also be great.
    let config_button_background = Rect::new(10, 500, width+20, height+10);
    let config_button_text = Rect::new(20, 505, width, height);

    let input_display_x: i32 = 20;
    let input_display_y: i32 = 20;

    let face_button_width: u32 = 30;

    let side_button_padding = 10;

    let bumper_width = face_button_width * 2;
    let bumper_height = face_button_width/2;

    let face_button_y_offset = input_display_y + 2*side_button_padding + bumper_height as i32 + face_button_width as i32;

    let middle_button_width: u32 = 15;
    let middle_buttons_x_offset = input_display_x + middle_button_width as i32 + 3 * face_button_width as i32;
    let middle_buttons_y_offset = face_button_y_offset + face_button_width as i32;

    let right_x_offset = middle_buttons_x_offset + 6 * middle_button_width as i32;

    let mut input_display_boxes = HashMap::new();
    input_display_boxes.insert(
        VigemInput::LeftTrigger,
        InputDisplay { rect: Rect::new(input_display_x, input_display_y, face_button_width * 2, face_button_width) }
    );
    input_display_boxes.insert(
        VigemInput::RightTrigger,
        InputDisplay { rect: Rect::new(right_x_offset + face_button_width as i32, input_display_y, face_button_width * 2, face_button_width) }
    );
    input_display_boxes.insert(
        VigemInput::Button(XButtons::LB),
        InputDisplay { rect: Rect::new(input_display_x, input_display_y + face_button_width as i32 + side_button_padding, bumper_width, bumper_height) }
    );
    input_display_boxes.insert(
        VigemInput::Button(XButtons::RB),
        InputDisplay { rect: Rect::new(right_x_offset + face_button_width as i32, input_display_y + face_button_width as i32 + side_button_padding, bumper_width, bumper_height) }
    );

    input_display_boxes.insert(
        VigemInput::Button(XButtons::UP),
        InputDisplay { rect: Rect::new(input_display_x + face_button_width as i32, face_button_y_offset, face_button_width, face_button_width) }
    );
    input_display_boxes.insert(
        VigemInput::Button(XButtons::RIGHT),
        InputDisplay { rect: Rect::new(input_display_x + 2*face_button_width as i32, face_button_y_offset + face_button_width as i32, face_button_width, face_button_width) }
    );
    input_display_boxes.insert(
        VigemInput::Button(XButtons::DOWN),
        InputDisplay { rect: Rect::new(input_display_x + face_button_width as i32, face_button_y_offset + 2*face_button_width as i32, face_button_width, face_button_width) }
    );
    input_display_boxes.insert(
        VigemInput::Button(XButtons::LEFT),
        InputDisplay { rect: Rect::new(input_display_x, face_button_y_offset + face_button_width as i32, face_button_width, face_button_width) }
    );

    input_display_boxes.insert(
        VigemInput::Button(XButtons::BACK),
        InputDisplay { rect: Rect::new(middle_buttons_x_offset, middle_buttons_y_offset, middle_button_width, middle_button_width) }
    );
    input_display_boxes.insert(
        VigemInput::Button(XButtons::GUIDE),
        InputDisplay { rect: Rect::new(middle_buttons_x_offset + 2 * middle_button_width as i32, middle_buttons_y_offset, middle_button_width, middle_button_width) }
    );
    input_display_boxes.insert(
        VigemInput::Button(XButtons::START),
        InputDisplay { rect: Rect::new(middle_buttons_x_offset + 2 * 2 * middle_button_width as i32, middle_buttons_y_offset, middle_button_width, middle_button_width) }
    );


    input_display_boxes.insert(
        VigemInput::Button(XButtons::Y),
        InputDisplay { rect: Rect::new(right_x_offset + face_button_width as i32, face_button_y_offset , face_button_width, face_button_width) }
    );
    input_display_boxes.insert(
        VigemInput::Button(XButtons::B),
        InputDisplay { rect: Rect::new(right_x_offset + 2*face_button_width as i32, face_button_y_offset + face_button_width as i32, face_button_width, face_button_width) }
    );
    input_display_boxes.insert(
        VigemInput::Button(XButtons::A),
        InputDisplay { rect: Rect::new(right_x_offset + face_button_width as i32, face_button_y_offset + 2*face_button_width as i32, face_button_width, face_button_width) }
    );
    input_display_boxes.insert(
        VigemInput::Button(XButtons::X),
        InputDisplay { rect: Rect::new(right_x_offset, face_button_y_offset + face_button_width as i32, face_button_width, face_button_width) }
    );

    input_display_boxes.insert(
        VigemInput::Button(XButtons::LTHUMB),
        InputDisplay { rect: Rect::new(input_display_x + 3*face_button_width as i32, face_button_y_offset + 3*face_button_width as i32, face_button_width, face_button_width) }
    );
    input_display_boxes.insert(
        VigemInput::Button(XButtons::RTHUMB),
        InputDisplay { rect: Rect::new(right_x_offset - face_button_width as i32, face_button_y_offset + 3*face_button_width as i32, face_button_width, face_button_width) }
    );

    let mut new_input = true;

    info!("Initialization complete");
    let mut event_pump = sdl_context.event_pump().unwrap();
    'mainloop: loop {
        event_pump.pump_events();
        for event in event_pump.poll_iter() {
            match event {
                Event::MouseButtonDown { mouse_btn, x, y, window_id, .. } => {
                    if window_id == canvas.window().id() && matches!(mouse_btn, sdl3::mouse::MouseButton::Left) {
                        new_input = true;

                        if config_button_background.contains_point(sdl3::rect::Point::new(x as i32, y as i32)) {
                            info!("Detecting mashing configuration");
                            current_app_state = AppState::DetectConfig;
                        }
                    }
                }
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

                    new_input = true;
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
                }
                Event::ControllerButtonUp { which, button, .. } => {
                    debug!("controller up {}", button.string());

                    new_input = true;
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
                        new_input = true;

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
                let mut should_mash = false;
                for (_, val) in held_buttons.iter() {
                    if val.len() >= MAX_MASHING_KEY_COUNT as usize {
                        // check if all triggers are pressed and activate the mashing
                        should_mash = mashing_buttons.read().unwrap().iter().all(|button| val.contains(button));
                        if should_mash { break };
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
                        info!("Config set, now accepting input");
                        continue 'mainloop;
                    }
                }
            }
        }

        if new_input {
            // background
            canvas.set_draw_color(Color::RGB(68, 136, 120));
            canvas.clear();
            // config button
            canvas.set_draw_color(Color::RGB(108, 55, 81));
            canvas.fill_rect(config_button_background).expect("Failed rendering button");
            canvas.copy(&texture, None, config_button_text).unwrap();
            // input display button
            let mut max_held: Option<&Vec<VigemInput>> = None;
            let mut max_len: usize = 0;
            for (_, val) in held_buttons.iter() {
                if val.len() > max_len {
                    max_held = Some(val);
                    max_len = val.len();
                }
            }

            if let Some(held) = max_held {
                for (key, textbox) in &mut input_display_boxes {
                    let highlighted = held.contains(&key);

                    textbox.draw(&mut canvas, highlighted);
                }
            }
            else {
                for (_, textbox) in &mut input_display_boxes {
                    textbox.draw(&mut canvas, false);
                }
            }
            // present to screen
            canvas.present();
            new_input = false;
        }

        // only poll at 2000 Hz
        std::thread::sleep(std::time::Duration::from_micros(500));
    }

}
