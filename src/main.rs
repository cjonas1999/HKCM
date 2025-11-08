#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod livesplit_core;
mod text_masher;

use crate::text_masher::{
    text_masher, IS_MASHER_ACTIVE, MAX_MASHING_KEY_COUNT, SHOULD_TERMINATE_MASHER,
};
use log::LevelFilter;
use log::{debug, error, info};
use log4rs::append::console::ConsoleAppender;
use log4rs::append::rolling_file::policy::compound::roll::delete::DeleteRoller;
use log4rs::append::rolling_file::policy::compound::trigger::size::SizeTrigger;
use log4rs::append::rolling_file::policy::compound::CompoundPolicy;
use log4rs::config::{Appender, Config, Root};
use log4rs::encode::pattern::PatternEncoder;
use sdl3::event::Event;
use sdl3::gamepad;
use sdl3::pixels::Color;
use sdl3::rect::Rect;
use serde::{ser::SerializeStruct, Deserialize, Deserializer, Serialize, Serializer};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs::File;
use std::io::Write;
use std::sync::atomic::Ordering;
use std::sync::{Arc, RwLock};
use std::thread;
#[cfg(target_os = "linux")]
use {
    std::os::unix::net::UnixStream, uinput::event::absolute::Hat,
    uinput::event::absolute::Position, uinput::event::controller::GamePad, uinput::event::Code,
    uinput::event::Controller, uinput::Event::Absolute,
};

#[cfg(target_os = "windows")]
use {
    std::os::windows::ffi::OsStrExt,
    vigem_client::XButtons,
    windows::core::PCWSTR,
    windows::Win32::Foundation::CloseHandle,
    windows::Win32::Storage::FileSystem::{
        CreateFileW, FlushFileBuffers, WriteFile, FILE_GENERIC_WRITE, FILE_SHARE_READ,
        OPEN_EXISTING, SECURITY_ANONYMOUS,
    },
};

enum AppState {
    DetectConfig,
    AcceptingInput,
}

#[cfg(target_os = "windows")]
#[derive(Serialize, Deserialize)]
struct Settings {
    mashing_triggers: Vec<VigemInput>,
}

#[cfg(target_os = "linux")]
struct Settings {
    mashing_triggers: Vec<Controller>,
}

#[cfg(target_os = "linux")]
impl Serialize for Settings {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Convert each Controller to its code
        let codes: Vec<i32> = self
            .mashing_triggers
            .iter()
            .map(|ctrl| ctrl.code())
            .collect();

        let mut state = serializer.serialize_struct("Settings", 1)?;
        state.serialize_field("mashing_triggers", &codes)?;
        state.end()
    }
}

#[cfg(target_os = "linux")]
impl<'de> Deserialize<'de> for Settings {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Helper {
            mashing_triggers: Vec<i32>,
        }

        let helper = Helper::deserialize(deserializer)?;

        // Convert codes back into Controller objects
        let controllers: Vec<Controller> = helper
            .mashing_triggers
            .into_iter()
            .map(code_to_controller)
            .collect();

        Ok(Settings {
            mashing_triggers: controllers,
        })
    }
}

#[cfg(target_os = "linux")]
fn code_to_controller(code: i32) -> Controller {
    use uinput::event::controller::*;
    match code {
        0x133 => Controller::GamePad(GamePad::North),
        0x131 => Controller::GamePad(GamePad::East),
        0x130 => Controller::GamePad(GamePad::South),
        0x134 => Controller::GamePad(GamePad::West),
        0x13A => Controller::GamePad(GamePad::Select),
        0x13C => Controller::GamePad(GamePad::Mode),
        0x13B => Controller::GamePad(GamePad::Start),
        0x136 => Controller::GamePad(GamePad::TL),
        0x137 => Controller::GamePad(GamePad::TR),
        _ => uinput::event::controller::Controller::All,
    }
}

#[cfg(target_os = "windows")]
#[derive(PartialEq, Debug, Clone, Serialize, Deserialize, Hash, Eq)]
enum VigemInput {
    Button(u16),
    LeftTrigger,
    RightTrigger,
}

// #[cfg(target_os = "linux")]
// #[derive(Debug, Clone, Copy)]
// enum UInputOutput {
//     Key(uinput::event::keyboard::Key),
//     KeyPad(uinput::event::keyboard::KeyPad),
//     GamePad(GamePad),
// }

#[cfg(target_os = "windows")]
fn sdl_button_to_input(button: gamepad::Button) -> Option<VigemInput> {
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

#[cfg(target_os = "linux")]
fn sdl_button_to_input(button: gamepad::Button) -> Option<Controller> {
    match button {
        gamepad::Button::North => Some(Controller::GamePad(GamePad::North)),
        gamepad::Button::East => Some(Controller::GamePad(GamePad::East)),
        gamepad::Button::South => Some(Controller::GamePad(GamePad::South)),
        gamepad::Button::West => Some(Controller::GamePad(GamePad::West)),
        gamepad::Button::Back => Some(Controller::GamePad(GamePad::Select)),
        gamepad::Button::Guide => Some(Controller::GamePad(GamePad::Mode)),
        gamepad::Button::Start => Some(Controller::GamePad(GamePad::Start)),
        gamepad::Button::LeftShoulder => Some(Controller::GamePad(GamePad::TL)),
        gamepad::Button::RightShoulder => Some(Controller::GamePad(GamePad::TR)),
        _ => None,
    }
}

struct InputDisplay {
    rect: Rect,
}

static INPUT_DEFAULT_COLOR: Color = Color::RGB(110, 110, 110);
static INPUT_HELD_COLOR: Color = Color::RGB(170, 170, 170);

impl InputDisplay {
    fn draw(&self, canvas: &mut sdl3::render::WindowCanvas, highlight: bool) {
        if highlight {
            canvas.set_draw_color(INPUT_HELD_COLOR);
        } else {
            canvas.set_draw_color(INPUT_DEFAULT_COLOR);
        }
        canvas
            .fill_rect(self.rect)
            .expect("Failed rendering background");
    }

    fn outline(&self, canvas: &mut sdl3::render::WindowCanvas) {
        canvas.set_draw_color(Color::RGB(255, 0, 0));
        let frect: sdl3::render::FRect = sdl3::render::FRect::from(self.rect);
        canvas.draw_rect(frect).expect("Failed to outline rect");
    }
}

#[cfg(target_os = "windows")]
fn toggle_masher_overlay(active: bool) -> Result<(), Box<dyn std::error::Error>> {
    let mut command = None;
    if active {
        debug!("Showing masher overlay");
        command = Some("masher_active");
    } else {
        debug!("Hiding masher overlay");
        command = Some("masher_inactive");
    }
    let pipe_name = r"\\.\pipe\masher_overlay_v2.0.1-beta";
    let name_w: Vec<u16> = OsStr::new(pipe_name)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    unsafe {
        let handle = CreateFileW(
            PCWSTR(name_w.as_ptr()),
            FILE_GENERIC_WRITE.0,
            FILE_SHARE_READ,
            None,
            OPEN_EXISTING,
            SECURITY_ANONYMOUS,
            None,
        )?;

        let mut written = 0u32;
        WriteFile(
            handle,
            Some(command.unwrap().as_bytes()),
            Some(&mut written as *mut u32),
            None,
        )?;

        let _ = FlushFileBuffers(handle);
        let _ = CloseHandle(handle);
    }

    Ok(())
}

#[cfg(target_os = "linux")]
fn toggle_masher_overlay(active: bool) -> Result<(), Box<dyn std::error::Error>> {
    let mut stream = UnixStream::connect("/tmp/masher_overlay_2.0.1-beta.sock")?;
    let mut command = None;
    if active {
        debug!("Showing masher overlay");
        command = Some("masher_active");
    } else {
        debug!("Hiding masher overlay");
        command = Some("masher_inactive");
    }

    stream
        .write_all(command.unwrap().as_bytes())
        .expect("Failed to send command");
    Ok(())
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
        .build(
            log_file_path,
            Box::new(CompoundPolicy::new(
                Box::new(SizeTrigger::new(1024 * 1024 * 1)),
                Box::new(DeleteRoller::new()),
            )),
        )
        .unwrap();

    #[cfg(debug_assertions)]
    let log_level = LevelFilter::Debug;
    #[cfg(not(debug_assertions))]
    let log_level = LevelFilter::Info;

    let config = Config::builder()
        .appender(Appender::builder().build("console", Box::new(console_log_appender)))
        .appender(Appender::builder().build("file", Box::new(log_file_appender)))
        .build(
            Root::builder()
                .appender("console")
                .appender("file")
                .build(log_level),
        )
        .unwrap();

    log4rs::init_config(config).unwrap();

    let mut current_app_state = AppState::AcceptingInput;
    // Read from settings file
    let mut settings_path = base_path.clone();
    settings_path.push("HKCM_settings.json");

    #[cfg(target_os = "windows")]
    let default_config = Settings {
        mashing_triggers: vec![
            VigemInput::Button(XButtons::X),
            VigemInput::Button(XButtons::A),
            VigemInput::Button(XButtons::B),
        ],
    };

    #[cfg(target_os = "linux")]
    let default_config = Settings {
        mashing_triggers: vec![
            Controller::GamePad(GamePad::East),
            Controller::GamePad(GamePad::South),
            Controller::GamePad(GamePad::West),
        ],
    };

    let mut settings: Settings = if !settings_path.exists() {
        let json = serde_json::to_string_pretty(&default_config)
            .expect("Failed to convert config to json");
        let mut file = File::create(&settings_path).unwrap();
        file.write_all(json.as_bytes())
            .expect("Failed to write config to file");

        default_config
    } else {
        let file = File::open(&settings_path).unwrap();

        serde_json::from_reader(file).unwrap_or_else(|_| {
            error!("Failed to parse settings from config file");
            default_config
        })
    };

    // App state setup
    sdl3::hint::set("SDL_JOYSTICK_ALLOW_BACKGROUND_EVENTS", "1");
    let sdl_context = sdl3::init().unwrap();
    let gamepad_system = sdl_context.gamepad().unwrap();
    // we need a reference to an open gamepad for it to stay open
    let mut _opened_gamepads: HashMap<u32, sdl3::gamepad::Gamepad> = HashMap::new();

    #[cfg(target_os = "windows")]
    let mut held_buttons: HashMap<u32, Vec<VigemInput>> = HashMap::new();

    #[cfg(target_os = "windows")]
    let mashing_buttons: Arc<RwLock<Vec<VigemInput>>> =
        Arc::new(std::sync::RwLock::new(settings.mashing_triggers.clone()));

    #[cfg(target_os = "windows")]
    {
        let thread_mashing_buttons = Arc::clone(&mashing_buttons);

        thread::spawn(move || {
            // VIGEM setup
            let client = vigem_client::Client::connect().unwrap();
            let id = vigem_client::TargetId::XBOX360_WIRED;
            let mut target = vigem_client::Xbox360Wired::new(client, id);
            target
                .plugin()
                .expect("Failed to plugin virtual controller");
            target
                .wait_ready()
                .expect("Could not wait for virtual controller to ready");

            text_masher(
                |key_to_press| {
                    let mut gamepad_state = vigem_client::XGamepad::default();

                    if key_to_press < MAX_MASHING_KEY_COUNT {
                        let mash_buttons = thread_mashing_buttons.read().unwrap();
                        if let Some(press) = mash_buttons.get(key_to_press as usize) {
                            match press {
                                VigemInput::Button(b) => gamepad_state.buttons = XButtons(*b),
                                VigemInput::LeftTrigger => gamepad_state.left_trigger = u8::MAX,
                                VigemInput::RightTrigger => gamepad_state.right_trigger = u8::MAX,
                            }
                        }
                    }

                    target
                        .update(&gamepad_state)
                        .expect("Failed to update virtual controller while mashing");
                },
                toggle_masher_overlay,
            );
        });
    }

    #[cfg(target_os = "linux")]
    let mut held_buttons: HashMap<u32, Vec<Controller>> = HashMap::new();

    #[cfg(target_os = "linux")]
    let mashing_buttons: Arc<RwLock<Vec<Controller>>> =
        Arc::new(std::sync::RwLock::new(settings.mashing_triggers.clone()));

    #[cfg(target_os = "linux")]
    {
        let thread_mashing_buttons = Arc::clone(&mashing_buttons);

        thread::spawn(move || {
            let mut controller = uinput::default()
                .unwrap()
                .name("Overbind Virtual Gamepad")
                .unwrap()
                .event(Absolute(uinput::event::absolute::Absolute::Position(
                    Position::X,
                )))
                .unwrap()
                .min(-32768)
                .max(32767)
                .fuzz(0)
                .flat(0)
                .event(Absolute(uinput::event::absolute::Absolute::Position(
                    Position::Y,
                )))
                .unwrap()
                .min(-32768)
                .max(32767)
                .fuzz(0)
                .flat(0)
                .event(Absolute(uinput::event::absolute::Absolute::Position(
                    Position::RX,
                )))
                .unwrap()
                .min(-32768)
                .max(32767)
                .fuzz(0)
                .flat(0)
                .event(Absolute(uinput::event::absolute::Absolute::Position(
                    Position::RY,
                )))
                .unwrap()
                .min(-32768)
                .max(32767)
                .fuzz(0)
                .flat(0)
                .event(Absolute(uinput::event::absolute::Absolute::Hat(Hat::X0)))
                .unwrap()
                .min(-1)
                .max(1)
                .fuzz(0)
                .flat(0)
                .event(Absolute(uinput::event::absolute::Absolute::Hat(Hat::Y0)))
                .unwrap()
                .min(-1)
                .max(1)
                .fuzz(0)
                .flat(0)
                .event(Absolute(uinput::event::absolute::Absolute::Position(
                    Position::Z,
                )))
                .unwrap()
                .min(0)
                .max(1023)
                .fuzz(0)
                .flat(0)
                .event(Absolute(uinput::event::absolute::Absolute::Position(
                    Position::RZ,
                )))
                .unwrap()
                .min(0)
                .max(1023)
                .fuzz(0)
                .flat(0)
                .event(uinput::Event::Controller(
                    uinput::event::Controller::GamePad(GamePad::North),
                ))
                .unwrap()
                .event(uinput::Event::Controller(
                    uinput::event::Controller::GamePad(GamePad::South),
                ))
                .unwrap()
                .event(uinput::Event::Controller(
                    uinput::event::Controller::GamePad(GamePad::East),
                ))
                .unwrap()
                .event(uinput::Event::Controller(
                    uinput::event::Controller::GamePad(GamePad::West),
                ))
                .unwrap()
                .event(uinput::Event::Controller(
                    uinput::event::Controller::GamePad(GamePad::TL),
                ))
                .unwrap()
                .event(uinput::Event::Controller(
                    uinput::event::Controller::GamePad(GamePad::TR),
                ))
                .unwrap()
                .event(uinput::Event::Controller(
                    uinput::event::Controller::GamePad(GamePad::ThumbL),
                ))
                .unwrap()
                .event(uinput::Event::Controller(
                    uinput::event::Controller::GamePad(GamePad::ThumbR),
                ))
                .unwrap()
                .event(uinput::Event::Controller(
                    uinput::event::Controller::GamePad(GamePad::Select),
                ))
                .unwrap()
                .event(uinput::Event::Controller(
                    uinput::event::Controller::GamePad(GamePad::Start),
                ))
                .unwrap()
                .event(uinput::Event::Controller(
                    uinput::event::Controller::GamePad(GamePad::Mode),
                ))
                .unwrap()
                .create()
                .unwrap();

            text_masher(
                |key_to_press| {
                    let mut send_face_button_event = |button: &Controller, key_is_down: bool| {
                        if key_is_down {
                            controller
                                .press(button)
                                .expect("Failed to press virtual controller button while mashing");
                        } else {
                            controller.release(button).expect(
                                "Failed to release virtual controller button while mashing",
                            );
                        }
                    };

                    if key_to_press < MAX_MASHING_KEY_COUNT {
                        let mash_buttons = thread_mashing_buttons.read().unwrap();
                        mash_buttons.iter().enumerate().for_each(|(index, button)| {
                            send_face_button_event(button, key_to_press == index as u8);
                        });
                    }

                    controller
                        .synchronize()
                        .expect("Failed to update virtual controller while mashing");
                },
                toggle_masher_overlay,
            );
        });
    }

    // Initialize GUI
    let video_subsystem = sdl_context.video().unwrap();

    let window = video_subsystem
        .window("HKCM", 320, 300)
        .position_centered()
        .build()
        .unwrap();
    let mut canvas = window.into_canvas();
    let texture_creator = canvas.texture_creator();

    let ttf_context = sdl3::ttf::init().unwrap();
    const FONT_DATA: &[u8] = include_bytes!("../fonts/Roboto-Regular.ttf");
    let mut font_stream =
        sdl3::iostream::IOStream::from_bytes(FONT_DATA).expect("Failed to read font data");
    let font = ttf_context
        .load_font_from_iostream(font_stream, 30.0)
        .unwrap();
    font_stream =
        sdl3::iostream::IOStream::from_bytes(FONT_DATA).expect("Failed to read font data");
    let small_font = ttf_context
        .load_font_from_iostream(font_stream, 17.0)
        .unwrap();

    // Define Input Display
    let input_display_x: i32 = 20;
    let input_display_y: i32 = 20;

    let face_button_width: u32 = 30;

    let side_button_padding = 10;

    let bumper_width = face_button_width * 2;
    let bumper_height = face_button_width / 2;

    let face_button_y_offset =
        input_display_y + 2 * side_button_padding + bumper_height as i32 + face_button_width as i32;

    let middle_button_width: u32 = 15;
    let middle_buttons_x_offset =
        input_display_x + middle_button_width as i32 + 3 * face_button_width as i32;
    let middle_buttons_y_offset = face_button_y_offset + face_button_width as i32;

    let right_x_offset = middle_buttons_x_offset + 6 * middle_button_width as i32;

    let mut input_display_boxes = HashMap::new();
    input_display_boxes.insert(
        #[cfg(target_os = "windows")]
        VigemInput::LeftTrigger,
        #[cfg(target_os = "linux")]
        Controller::GamePad(GamePad::TL2),
        InputDisplay {
            rect: Rect::new(
                input_display_x,
                input_display_y,
                face_button_width * 2,
                face_button_width,
            ),
        },
    );
    input_display_boxes.insert(
        #[cfg(target_os = "windows")]
        VigemInput::RightTrigger,
        #[cfg(target_os = "linux")]
        Controller::GamePad(GamePad::TR2),
        InputDisplay {
            rect: Rect::new(
                right_x_offset + face_button_width as i32,
                input_display_y,
                face_button_width * 2,
                face_button_width,
            ),
        },
    );
    input_display_boxes.insert(
        #[cfg(target_os = "windows")]
        VigemInput::Button(XButtons::LB),
        #[cfg(target_os = "linux")]
        Controller::GamePad(GamePad::TL),
        InputDisplay {
            rect: Rect::new(
                input_display_x,
                input_display_y + face_button_width as i32 + side_button_padding,
                bumper_width,
                bumper_height,
            ),
        },
    );
    input_display_boxes.insert(
        #[cfg(target_os = "windows")]
        VigemInput::Button(XButtons::RB),
        #[cfg(target_os = "linux")]
        Controller::GamePad(GamePad::TR),
        InputDisplay {
            rect: Rect::new(
                right_x_offset + face_button_width as i32,
                input_display_y + face_button_width as i32 + side_button_padding,
                bumper_width,
                bumper_height,
            ),
        },
    );

    #[cfg(target_os = "windows")]
    input_display_boxes.insert(
        VigemInput::Button(XButtons::UP),
        InputDisplay {
            rect: Rect::new(
                input_display_x + face_button_width as i32,
                face_button_y_offset,
                face_button_width,
                face_button_width,
            ),
        },
    );
    #[cfg(target_os = "windows")]
    input_display_boxes.insert(
        VigemInput::Button(XButtons::RIGHT),
        InputDisplay {
            rect: Rect::new(
                input_display_x + 2 * face_button_width as i32,
                face_button_y_offset + face_button_width as i32,
                face_button_width,
                face_button_width,
            ),
        },
    );
    #[cfg(target_os = "windows")]
    input_display_boxes.insert(
        VigemInput::Button(XButtons::DOWN),
        InputDisplay {
            rect: Rect::new(
                input_display_x + face_button_width as i32,
                face_button_y_offset + 2 * face_button_width as i32,
                face_button_width,
                face_button_width,
            ),
        },
    );
    #[cfg(target_os = "windows")]
    input_display_boxes.insert(
        VigemInput::Button(XButtons::LEFT),
        InputDisplay {
            rect: Rect::new(
                input_display_x,
                face_button_y_offset + face_button_width as i32,
                face_button_width,
                face_button_width,
            ),
        },
    );

    input_display_boxes.insert(
        #[cfg(target_os = "windows")]
        VigemInput::Button(XButtons::BACK),
        #[cfg(target_os = "linux")]
        Controller::GamePad(GamePad::Select),
        InputDisplay {
            rect: Rect::new(
                middle_buttons_x_offset,
                middle_buttons_y_offset,
                middle_button_width,
                middle_button_width,
            ),
        },
    );
    input_display_boxes.insert(
        #[cfg(target_os = "windows")]
        VigemInput::Button(XButtons::GUIDE),
        #[cfg(target_os = "linux")]
        Controller::GamePad(GamePad::Mode),
        InputDisplay {
            rect: Rect::new(
                middle_buttons_x_offset + 2 * middle_button_width as i32,
                middle_buttons_y_offset,
                middle_button_width,
                middle_button_width,
            ),
        },
    );
    input_display_boxes.insert(
        #[cfg(target_os = "windows")]
        VigemInput::Button(XButtons::START),
        #[cfg(target_os = "linux")]
        Controller::GamePad(GamePad::Start),
        InputDisplay {
            rect: Rect::new(
                middle_buttons_x_offset + 2 * 2 * middle_button_width as i32,
                middle_buttons_y_offset,
                middle_button_width,
                middle_button_width,
            ),
        },
    );

    input_display_boxes.insert(
        #[cfg(target_os = "windows")]
        VigemInput::Button(XButtons::Y),
        #[cfg(target_os = "linux")]
        Controller::GamePad(GamePad::North),
        InputDisplay {
            rect: Rect::new(
                right_x_offset + face_button_width as i32,
                face_button_y_offset,
                face_button_width,
                face_button_width,
            ),
        },
    );
    input_display_boxes.insert(
        #[cfg(target_os = "windows")]
        VigemInput::Button(XButtons::B),
        #[cfg(target_os = "linux")]
        Controller::GamePad(GamePad::East),
        InputDisplay {
            rect: Rect::new(
                right_x_offset + 2 * face_button_width as i32,
                face_button_y_offset + face_button_width as i32,
                face_button_width,
                face_button_width,
            ),
        },
    );
    input_display_boxes.insert(
        #[cfg(target_os = "windows")]
        VigemInput::Button(XButtons::A),
        #[cfg(target_os = "linux")]
        Controller::GamePad(GamePad::South),
        InputDisplay {
            rect: Rect::new(
                right_x_offset + face_button_width as i32,
                face_button_y_offset + 2 * face_button_width as i32,
                face_button_width,
                face_button_width,
            ),
        },
    );
    input_display_boxes.insert(
        #[cfg(target_os = "windows")]
        VigemInput::Button(XButtons::X),
        #[cfg(target_os = "linux")]
        Controller::GamePad(GamePad::West),
        InputDisplay {
            rect: Rect::new(
                right_x_offset,
                face_button_y_offset + face_button_width as i32,
                face_button_width,
                face_button_width,
            ),
        },
    );

    let thumbstick_button_y_offset = face_button_y_offset + 3 * face_button_width as i32;
    input_display_boxes.insert(
        #[cfg(target_os = "windows")]
        VigemInput::Button(XButtons::LTHUMB),
        #[cfg(target_os = "linux")]
        Controller::GamePad(GamePad::ThumbL),
        InputDisplay {
            rect: Rect::new(
                input_display_x + 3 * face_button_width as i32,
                thumbstick_button_y_offset,
                face_button_width,
                face_button_width,
            ),
        },
    );
    input_display_boxes.insert(
        #[cfg(target_os = "windows")]
        VigemInput::Button(XButtons::RTHUMB),
        #[cfg(target_os = "linux")]
        Controller::GamePad(GamePad::ThumbR),
        InputDisplay {
            rect: Rect::new(
                right_x_offset - face_button_width as i32,
                thumbstick_button_y_offset,
                face_button_width,
                face_button_width,
            ),
        },
    );

    // Define config button
    let configure_text_surface = font
        .render("Configure")
        .blended(Color::RGBA(250, 250, 250, 255))
        .map_err(|e| e.to_string())
        .unwrap();
    let configure_texture = texture_creator
        .create_texture_from_surface(&configure_text_surface)
        .map_err(|e| e.to_string())
        .unwrap();
    let sdl3::render::TextureQuery {
        width: configure_width,
        height: configure_height,
        ..
    } = configure_texture.query();

    let cancel_text_surface = font
        .render("Cancel")
        .blended(Color::RGBA(250, 250, 250, 255))
        .map_err(|e| e.to_string())
        .unwrap();
    let cancel_texture = texture_creator
        .create_texture_from_surface(&cancel_text_surface)
        .map_err(|e| e.to_string())
        .unwrap();
    let sdl3::render::TextureQuery {
        width: cancel_width,
        height: cancel_height,
        ..
    } = cancel_texture.query();

    let config_button_y_offset = thumbstick_button_y_offset + 50;
    let config_text_padding = 10;
    let config_button_background = Rect::new(
        input_display_x,
        config_button_y_offset,
        configure_width + 2 * config_text_padding,
        configure_height + 2 * config_text_padding,
    );
    let config_button_text = Rect::new(
        input_display_x + config_text_padding as i32,
        config_button_y_offset + config_text_padding as i32,
        configure_width,
        configure_height,
    );

    let cancel_text_padding_x = (config_button_background.width() - cancel_width) / 2;
    let cancel_button_text = Rect::new(
        input_display_x + cancel_text_padding_x as i32,
        config_button_y_offset + config_text_padding as i32,
        cancel_width,
        cancel_height,
    );

    let guide_text_surface = small_font
        .render("Hold 3 buttons\nto configure\nmasher triggers.")
        .blended_wrapped(Color::RGBA(250, 250, 250, 255), 0)
        .map_err(|e| e.to_string())
        .unwrap();
    let guide_texture = texture_creator
        .create_texture_from_surface(&guide_text_surface)
        .map_err(|e| e.to_string())
        .unwrap();
    let sdl3::render::TextureQuery {
        width: guide_width,
        height: guide_height,
        ..
    } = guide_texture.query();
    let guide_x = config_button_background.x() + config_button_background.width() as i32 + 8;
    let guide_text = Rect::new(guide_x, config_button_y_offset, guide_width, guide_height);

    info!("Initialization complete");
    let mut new_input = true;
    let mut event_pump = sdl_context.event_pump().unwrap();
    'mainloop: loop {
        event_pump.pump_events();
        for event in event_pump.poll_iter() {
            match event {
                Event::MouseButtonDown {
                    mouse_btn,
                    x,
                    y,
                    window_id,
                    ..
                } => {
                    if window_id == canvas.window().id()
                        && matches!(mouse_btn, sdl3::mouse::MouseButton::Left)
                    {
                        new_input = true;

                        if config_button_background
                            .contains_point(sdl3::rect::Point::new(x as i32, y as i32))
                        {
                            if matches!(current_app_state, AppState::AcceptingInput) {
                                info!("Detecting mashing configuration");
                                current_app_state = AppState::DetectConfig;
                            } else if matches!(current_app_state, AppState::DetectConfig) {
                                info!("Cancel detection");
                                current_app_state = AppState::AcceptingInput;
                            }
                        }
                    }
                }
                Event::ControllerDeviceAdded { which, .. } => {
                    if let Ok(gamepad) = gamepad_system.open(which) {
                        _opened_gamepads.insert(which, gamepad);
                    }
                }
                Event::ControllerDeviceRemoved { which, .. } => {
                    _opened_gamepads.remove(&which);
                }
                Event::ControllerButtonDown { which, button, .. } => {
                    debug!("controller down {}", button.string());

                    new_input = true;
                    if let Some(input) = sdl_button_to_input(button) {
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
                        if let Some(input) = sdl_button_to_input(button) {
                            entry.retain(|held| *held != input);

                            if entry.is_empty() {
                                held_buttons.remove_entry(&which);
                            }
                        }
                    }
                }

                Event::ControllerAxisMotion {
                    which, axis, value, ..
                } => {
                    #[cfg(target_os = "windows")]
                    let converted_input = match axis {
                        gamepad::Axis::TriggerLeft => Some(VigemInput::LeftTrigger),
                        gamepad::Axis::TriggerRight => Some(VigemInput::RightTrigger),
                        _ => None,
                    };

                    #[cfg(target_os = "linux")]
                    let converted_input = match axis {
                        gamepad::Axis::TriggerLeft => Some(Controller::GamePad(GamePad::TL2)),
                        gamepad::Axis::TriggerRight => Some(Controller::GamePad(GamePad::TR2)),
                        _ => None,
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
                        } else {
                            if let Some(entry) = held_buttons.get_mut(&which) {
                                entry.retain(|held| *held != input);

                                if entry.is_empty() {
                                    held_buttons.remove_entry(&which);
                                }
                            }
                        }
                    }
                }

                Event::Quit { .. } => {
                    SHOULD_TERMINATE_MASHER.store(true, Ordering::SeqCst);
                    break 'mainloop;
                }
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
                        should_mash = mashing_buttons
                            .read()
                            .unwrap()
                            .iter()
                            .all(|button| val.contains(button));
                        if should_mash {
                            break;
                        };
                    }
                }

                if IS_MASHER_ACTIVE.load(Ordering::SeqCst) != should_mash {
                    debug!("all mashing triggers pressed: {}", should_mash);
                    IS_MASHER_ACTIVE.store(should_mash, Ordering::SeqCst);
                }
            } else if matches!(current_app_state, AppState::DetectConfig) {
                for (_, val) in held_buttons.iter() {
                    if val.len() == MAX_MASHING_KEY_COUNT as usize {
                        {
                            *mashing_buttons
                                .write()
                                .expect("Failed to get state while storing config") = val.clone();
                        }

                        current_app_state = AppState::AcceptingInput;

                        settings.mashing_triggers = val.clone();

                        let json = serde_json::to_string_pretty(&settings)
                            .expect("Failed to convert config to json");
                        let mut file = File::create(&settings_path).unwrap();
                        file.write_all(json.as_bytes())
                            .expect("Failed to write config to file");
                        info!("Config set, now accepting input");
                        continue 'mainloop;
                    }
                }
            }
        }

        // Render GUI
        if new_input {
            // Draw background
            canvas.set_draw_color(Color::RGB(106, 166, 180));
            canvas.clear();

            // Draw config button
            if matches!(current_app_state, AppState::AcceptingInput) {
                canvas.set_draw_color(Color::RGB(70, 87, 117));
                canvas
                    .fill_rect(config_button_background)
                    .expect("Failed rendering button");
                canvas
                    .copy(&configure_texture, None, config_button_text)
                    .unwrap();
            } else if matches!(current_app_state, AppState::DetectConfig) {
                canvas.set_draw_color(Color::RGB(93, 114, 152));
                canvas
                    .fill_rect(config_button_background)
                    .expect("Failed rendering button");
                canvas
                    .copy(&cancel_texture, None, cancel_button_text)
                    .unwrap();
                canvas.copy(&guide_texture, None, guide_text).unwrap();
            }

            // Draw input display
            #[cfg(target_os = "windows")]
            let mut max_held: Option<&Vec<VigemInput>> = None;
            #[cfg(target_os = "linux")]
            let mut max_held: Option<&Vec<Controller>> = None;
            let mut max_len: usize = 0;
            for (_, val) in held_buttons.iter() {
                if val.len() > max_len {
                    max_held = Some(val);
                    max_len = val.len();
                }
            }

            if let Some(held) = max_held {
                for (key, display) in &mut input_display_boxes {
                    let highlighted = held.contains(&key);
                    display.draw(&mut canvas, highlighted);
                }
            } else {
                for (_, display) in &mut input_display_boxes {
                    display.draw(&mut canvas, false);
                }
            }

            // Outline configured mashing triggers
            for mashing_button in mashing_buttons.read().unwrap().iter() {
                if let Some(display) = input_display_boxes.get_mut(mashing_button) {
                    display.outline(&mut canvas);
                }
            }

            canvas.present();
            new_input = false;
        }

        // only poll at 2000 Hz
        std::thread::sleep(std::time::Duration::from_micros(500));
    }
}
