//#![windows_subsystem = "windows"]

use dirs::data_local_dir;
use iced::{executor, window, Align, Application, Element, Settings, Subscription};

use log::{log_enabled, LevelFilter};
use player::{PlayerCommand, PlayerStatus};

pub mod prelude {
    pub use log::{debug, error, info, trace, warn};
    pub use stable_eyre::eyre::{eyre, Report, WrapErr};
    pub type Result<T> = std::result::Result<T, Report>;
}
mod player;
mod playlist;

use prelude::*;

use iced_native::{
    button, slider, Button, Column, Command, HorizontalAlignment, Length, Row, Slider, Text,
};
use rand::prelude::*;

use serde::Deserialize;
use serde::Serialize;
use std::thread;
use std::{
    borrow::Cow,
    collections::HashMap,
    fs::File,
    io::Write,
    path::PathBuf,
    sync::mpsc::{Receiver, Sender},
    time::Duration,
};
use std::{collections::HashSet, thread::JoinHandle};

const SAVE_INTERVAL: Duration = Duration::from_secs(60 * 30);

#[derive(Serialize, Deserialize, Default)]
struct ConfigData<'a> {
    playlists: Cow<'a, HashMap<PathBuf, Vec<String>>>,
    favorites: Cow<'a, HashSet<String>>,
    volume: u8,
    path: PathBuf,
    current_playlist: Cow<'a, String>,
}

struct PlaybackControl {
    path: PathBuf,
    play_next: button::State,
    is_paused: bool,
    pause: button::State,
    favorite: button::State,
    export_favorites: button::State,
    trash_current: button::State,
    data_favorites: HashSet<String>,
    is_favorite: bool,
    volume_input: slider::State,
    volume: u8,
    length: Option<Duration>,
    playtime: Option<Duration>,
    total_playtime: Option<Duration>,
    tx: Sender<PlayerCommand>,
    rx: Receiver<PlayerStatus>,
    current_playlist: String,
    /// Displayed current file,
    /// also used by play_next to remove the current file from the playlist, if this is not empty
    current_file: String,
    playlists: HashMap<PathBuf, Vec<String>>,
    child: JoinHandle<()>,
}

impl PlaybackControl {
    fn play_next(&mut self) {
        let mut remove = false;
        if let Some(v) = self.playlists.get_mut(&self.path) {
            if !v.is_empty() {
                if !self.current_file.is_empty() {
                    let removed = v.remove(0);
                    trace!("Removing {}", removed);
                }
            }
            if !v.is_empty() {
                self.tx
                    .send(PlayerCommand::Play(v[0].clone(), self.volume))
                    .expect("Can't send playback command!");
                self.current_playlist = self.path.to_string_lossy().into_owned();
            } else {
                remove = true;
            }
        }
        if remove {
            debug!("Removing playlist");
            self.playlists.remove(&self.path);
        }
    }

    fn store_state(&self) {
        let data = ConfigData {
            playlists: Cow::Borrowed(&self.playlists),
            volume: self.volume,
            current_playlist: Cow::Borrowed(&self.current_playlist),
            path: self.path.clone(),
            favorites: Cow::Borrowed(&self.data_favorites),
        };
        match serde_json::to_string(&data) {
            Err(e) => warn!("Can't serialize data! {}", e),
            Ok(v) => {
                thread::spawn(move || {
                    let file = config_path(true);
                    match File::create(&file) {
                        Err(e) => warn!("Can't create config file {:?}: {}", file, e),
                        Ok(mut file) => match file.write_all(v.as_bytes()) {
                            Err(e) => warn!("Error writing config {}", e),
                            Ok(_) => match std::fs::rename(config_path(true), config_path(false)) {
                                Ok(_) => info!("Config saved"),
                                Err(e) => error!("Can't move file over backup: {}", e),
                            },
                        },
                    }
                });
            }
        }
    }

    /// Handle time tick for updating UI from player state updates
    fn handle_tick(&mut self) {
        if let Ok(msg) = self.rx.try_recv() {
            if log_enabled!(log::Level::Trace) {
                match msg {
                    PlayerStatus::Playtime(_) => (),
                    _ => trace!("Player state: {:?}", msg),
                }
            }
            match msg {
                PlayerStatus::Playing(f, length) => {
                    self.current_file = f;
                    self.is_paused = false;
                    self.is_favorite = self.data_favorites.contains(&self.current_file);
                    debug!("Length {:?}", length);
                    self.length = length;
                }
                PlayerStatus::Ended => {
                    debug!("Playback ended");
                    self.play_next();
                    self.current_file = String::new();
                }
                PlayerStatus::Paused => {
                    self.is_paused = true;
                }
                PlayerStatus::Playtime(time) => {
                    self.playtime = time;
                }
                PlayerStatus::InvalidFile(f) => {
                    dbg!(&f);
                    // set as file, so play_next removes it
                    self.current_file = f;
                    self.play_next();
                    self.current_file = String::new();
                }
            }
        }
    }

    fn file_dropped(&mut self, file: PathBuf) {
        match std::fs::read_to_string(&file) {
            Ok(data) => match playlist_decoder::decode(&data) {
                Ok(mut playlist) => {
                    if let Some(v) = self.playlists.get_mut(&file) {
                        if v.is_empty() {
                            v.append(&mut playlist);
                        }
                    } else {
                        playlist.shuffle(&mut thread_rng());
                        self.playlists.insert(file.clone(), playlist);
                    }
                    self.path = file;
                    // reset current_file to not remove this file from playback
                    self.current_file = String::new();
                    self.play_next();
                }
                Err(e) => error!("{}", e),
            },
            Err(e) => warn!("Can't open dropped file {}", e),
        }
    }

    fn trash_file(&mut self) {
        if !self.current_file.is_empty() {
            match trash::delete(&self.current_file) {
                Ok(_) => info!("Trashed {}", self.current_file),
                Err(e) => error!("Can't trash file {}: {}", self.current_file, e),
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    PlayNext,
    Pause,
    SliderChanged(u8),
    Window(iced_native::Event),
    Tick,
    ToggleFavorite,
    ExportFavorites,
    SaveConfig,
    TrashFile,
}

/// Config file, temp specifies if a .bak version should be used
fn config_path(temp: bool) -> PathBuf {
    let mut file = data_local_dir().unwrap();
    match temp {
        true => file.push("audio_wrench.json.bak"),
        false => file.push("audio_wrench.json"),
    }
    file
}

impl Default for PlaybackControl {
    fn default() -> Self {
        let file = config_path(false);
        let data: ConfigData = if file.is_file() {
            match std::fs::read_to_string(&file)
                .map_err(Report::from)
                .and_then(|v| serde_json::from_str(&v).map_err(Report::from))
            {
                Ok(v) => v,
                Err(e) => {
                    error!("Unable to read config at {:?}: {}", file, e);
                    Default::default()
                }
            }
        } else {
            Default::default()
        };
        let (tx, rx, child) = player::Player::new().expect("Can't start audio controller");
        // TODO: don't use into_owned, avoid copy
        Self {
            path: data.path,
            play_next: Default::default(),
            pause: Default::default(),
            volume_input: Default::default(),
            favorite: Default::default(),
            trash_current: Default::default(),
            export_favorites: Default::default(),
            volume: data.volume,
            tx,
            rx,
            playlists: data.playlists.into_owned(),
            current_playlist: data.current_playlist.into_owned(),
            current_file: Default::default(),
            is_favorite: false,
            is_paused: false,
            data_favorites: data.favorites.into_owned(),
            length: None,
            total_playtime: None,
            playtime: None,
            child,
        }
    }
}

impl Drop for PlaybackControl {
    fn drop(&mut self) {
        self.store_state();
    }
}

impl Application for PlaybackControl {
    type Executor = executor::Default;
    type Message = Message;
    type Flags = ();
    fn view(&mut self) -> Element<Self::Message> {
        let fav_text = match self.is_favorite {
            true => "Unfavorite",
            false => "Favorite",
        };
        let play_text = match self.current_file.is_empty() {
            true => "Play",
            false => "Next",
        };
        let pause_text = match self.is_paused {
            true => "Resume",
            false => "Pause",
        };

        let length_text = match self.length {
            None => String::from("--:--"),
            Some(v) => {
                let secs_total = v.as_secs();
                let minutes = secs_total / 60;
                format!("{:02}:{:02}", minutes, secs_total - (minutes * 60))
            }
        };
        let playtime_text = match self.playtime {
            None => String::from("--:--"),
            Some(v) => {
                let secs_total = v.as_secs();
                let minutes = secs_total / 60;
                format!("{:02}:{:02}", minutes, secs_total - (minutes * 60))
            }
        };
        let timer_text = format!("{}/{}", playtime_text, length_text);
        let mut row_controls = Row::new()
            .align_items(Align::Center)
            .spacing(20)
            .push(
                Button::new(&mut self.play_next, Text::new(play_text)).on_press(Message::PlayNext),
            )
            .push(Button::new(&mut self.pause, Text::new(pause_text)).on_press(Message::Pause));

        if !self.current_file.is_empty() {
            row_controls = row_controls
                .push(
                    Button::new(&mut self.favorite, Text::new(fav_text))
                        .on_press(Message::ToggleFavorite),
                )
                .push(
                    Button::new(&mut self.trash_current, Text::new("Trash File"))
                        .on_press(Message::TrashFile),
                );
        }

        Column::new()
            .max_width(800)
            .spacing(20)
            .align_items(Align::Center)
            .push(
                Text::new(&self.current_playlist.to_string())
                    .size(20)
                    .width(Length::Fill)
                    .horizontal_alignment(HorizontalAlignment::Center),
            )
            .push(
                Text::new(&self.current_file.to_string())
                    .size(20)
                    .width(Length::Fill)
                    .horizontal_alignment(HorizontalAlignment::Center),
            )
            .push(row_controls)
            .push(
                Text::new(timer_text)
                    .size(20)
                    .width(Length::Fill)
                    .horizontal_alignment(HorizontalAlignment::Center),
            )
            .push(
                Text::new(format!("{}% Volume", self.volume))
                    .size(20)
                    .width(Length::Fill)
                    .horizontal_alignment(HorizontalAlignment::Center),
            )
            // TODO: use https://crates.io/crates/iced_audio control elements
            .push(Slider::new(
                &mut self.volume_input,
                0..=100,
                self.volume,
                Message::SliderChanged,
            ))
            .padding(20)
            .push(
                Text::new("Drop a playlist file to start (.m3u/.pls/.xspf/.asx)")
                    .size(20)
                    .width(Length::Fill)
                    .horizontal_alignment(HorizontalAlignment::Center),
            )
            .padding(20)
            .push(
                Button::new(&mut self.export_favorites, Text::new("Export Favorites"))
                    .on_press(Message::ExportFavorites),
            )
            .into()
    }

    fn update(&mut self, message: Message) -> Command<Self::Message> {
        match message {
            Message::PlayNext => {
                self.play_next();
            }
            Message::Pause => {
                self.tx
                    .send(PlayerCommand::Pause)
                    .expect("Can't send playback command!");
            }
            Message::SliderChanged(v) => {
                self.volume = v;
                self.tx
                    .send(PlayerCommand::Volume(v))
                    .expect("Can't send playback command!");
            }
            Message::Window(iced_native::Event::Window(
                iced_native::window::Event::FileDropped(f),
            )) => self.file_dropped(f),
            Message::Tick => self.handle_tick(),
            Message::Window(_) => (),
            Message::ToggleFavorite => {
                if !self.current_file.is_empty() {
                    if self.is_favorite {
                        self.data_favorites.remove(&self.current_file);
                    } else {
                        self.data_favorites.insert(self.current_file.clone());
                    }
                    self.is_favorite = !self.is_favorite;
                }
            }
            Message::ExportFavorites => {
                let path = "favorites.xspf";
                match playlist::write_playlist(self.data_favorites.iter(), path) {
                    Ok(_) => info!("Favorites written to {}", path),
                    Err(e) => error!("Can't write favorites to {}: {}", path, e),
                }
            }
            Message::SaveConfig => {
                self.store_state();
            }
            Message::TrashFile => self.trash_file(),
        }
        Command::none()
    }

    fn new(_flags: ()) -> (PlaybackControl, Command<Message>) {
        (PlaybackControl::default(), Command::none())
    }

    fn title(&self) -> String {
        String::from("Audio Wrench")
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        let timer_ticks = iced::time::every(Duration::from_millis(100)).map(|_| Message::Tick);
        let timer_save = iced::time::every(SAVE_INTERVAL).map(|_| Message::SaveConfig);
        let window_ticks = iced_native::subscription::events().map(Message::Window);
        Subscription::batch(vec![window_ticks, timer_ticks, timer_save])
    }

    fn background_color(&self) -> iced_native::Color {
        iced_native::Color::WHITE
    }

    fn scale_factor(&self) -> f64 {
        1.0
    }

    fn mode(&self) -> iced::window::Mode {
        iced::window::Mode::Windowed
    }
}

fn main() -> Result<()> {
    stable_eyre::install().expect("Can't initialize backtrace handling!");
    let mut builder = env_logger::Builder::new();
    builder.filter_level(LevelFilter::Warn);
    #[cfg(debug_assertions)]
    builder.filter_module("audio_wrench", LevelFilter::Trace);
    #[cfg(not(debug_assertions))]
    builder.filter_module("audio_wrench", LevelFilter::Info);
    builder.parse_env("RUST_LOG");
    builder.init();

    let mut settings: Settings<()> = Settings::default();
    let mut window_settings = window::Settings::default();
    window_settings.size = (500, 500);
    settings.window = window_settings;
    PlaybackControl::run(settings).expect("Failed to run GUI");

    Ok(())
}
