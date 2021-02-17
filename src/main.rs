//#![windows_subsystem = "windows"]

use dirs::data_local_dir;
use iced::{executor, window, Align, Application, Element, Settings, Subscription};

use url::Url;

pub mod prelude {
    pub use log::{error, info, warn,trace,debug};
    pub use stable_eyre::eyre::{eyre, Report, WrapErr};
    pub type Result<T> = std::result::Result<T, Report>;
}
mod playlist;

use prelude::*;

use iced_native::{
    button, renderer, slider, Button, Column, Command, HorizontalAlignment, Length, Row, Slider,
    Text,
};
use rand::prelude::*;
use rodio::{OutputStream, OutputStreamHandle, Sink, Source};
use serde::Deserialize;
use serde::Serialize;
use std::{collections::HashSet, io::BufReader, thread::JoinHandle};
use std::thread;
use std::{
    borrow::Cow,
    collections::HashMap,
    fs::File,
    io::Write,
    path::PathBuf,
    sync::mpsc::{Receiver, Sender, TryRecvError},
    time::Duration,
};

use std::env;
use std::sync::{mpsc::channel, Arc, Mutex};

#[derive(Serialize, Deserialize, Default)]
struct ConfigData<'a> {
    playlists: Cow<'a, HashMap<PathBuf, Vec<String>>>,
    favorites: Cow<'a,HashSet<String>>,
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
                    v.remove(0);
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
            println!("Removing playlist");
            self.playlists.remove(&self.path);
        }
    }

    fn store_state(&self) -> Result<()> {
        let data = ConfigData {
            playlists: Cow::Borrowed(&self.playlists),
            volume: self.volume,
            current_playlist: Cow::Borrowed(&self.current_playlist),
            path: self.path.clone(),
            favorites: Cow::Borrowed(&self.data_favorites),
        };
        let file = config_path();
        let v = serde_json::to_string(&data)?;
        let mut file = File::create(file)?;
        file.write_all(v.as_bytes())?;
        Ok(())
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
}

enum PlayerCommand {
    Volume(u8),
    Play(String, u8),
    Pause,
}

enum PlayerStatus {
    Playing(String,Option<Duration>),
    Ended,
    Paused,
}

fn calc_volume(v: u8) -> f32 {
    (v as f32) / 100.0
}

fn spawn_audio() -> Result<(Sender<PlayerCommand>, Receiver<PlayerStatus>,JoinHandle<()>)> {
    let (tx, rx) = channel::<PlayerCommand>();
    let (update_tx, state_rx) = channel::<PlayerStatus>();
    let child = thread::Builder::new().name("audio controller".to_string()).spawn(move || {
        let (_stream, stream_handle) = rodio::OutputStream::try_default().unwrap();
        let mut sink: Option<Sink> = None;
        let mut last_file: String = Default::default();
        let mut ended = false;
        let mut length: Option<Duration> = None;
        loop {
            match rx.try_recv() {
                Ok(msg) => match msg {
                    PlayerCommand::Volume(v) => {
                        if let Some(ref sink) = sink {
                            sink.set_volume(calc_volume(v));
                        }
                    }
                    PlayerCommand::Play(path, volume) => {
                        ended = false;
                        if let Some(ref v) = sink {
                            v.stop();
                        }
                        let path = match Url::parse(&path) {
                            Ok(v) => match v.to_file_path() {
                                Ok(v) => v,
                                Err(_) => {
                                    warn!("Can't play URLs, skipping");
                                    continue;
                                }
                            },
                            Err(_e) => path.into(),
                        };
                        match std::fs::File::open(&path) {
                            Ok(file) => {
                                println!("Starting playback");
                                last_file = path.to_string_lossy().into_owned();
                                let input = match rodio::Decoder::new(file) {
                                    Ok(v) => v,
                                    Err(e) => {warn!("Can't play {:?} unsupported format?",e); continue;}
                                };
                                let length = input.total_duration();
                                dbg!(input.size_hint());
                                let new_sink = Sink::try_new(&stream_handle).expect("Can't open new playback-sink!");
                                new_sink.set_volume(calc_volume(volume));
                                new_sink.append(input);
                                sink = Some(new_sink);
                                update_tx
                                    .send(PlayerStatus::Playing(last_file.clone(),length))
                                    .expect("Can't send playback status!");
                            }
                            Err(e) => warn!("{:?} {}", path, e),
                        }
                    }
                    PlayerCommand::Pause => {
                        ended = false;
                        if let Some(ref mut sink) = sink {
                            if sink.is_paused() {
                                sink.play();
                                update_tx
                                    .send(PlayerStatus::Playing(last_file.clone(),length))
                                    .expect("Can't send playback status!");
                            } else {
                                sink.pause();
                                update_tx.send(PlayerStatus::Paused).expect("Can't send playback status!");
                            }
                        }
                    }
                },
                Err(TryRecvError::Empty) => {
                    if sink.as_ref().map_or(true, |v| v.empty()) && !ended {
                        update_tx.send(PlayerStatus::Ended).expect("Can't send playback status!");
                        ended = true;
                    } else {
                        thread::sleep(Duration::from_millis(100));
                    }
                }
                Err(TryRecvError::Disconnected) => {
                    break;
                }
            }
        }
    })?;
    Ok((tx, state_rx,child))
}

fn config_path() -> PathBuf {
    let mut file = data_local_dir().unwrap();
    file.push("audio_wrench.json");
    file
}

impl Default for PlaybackControl {
    fn default() -> Self {
        let file = config_path();
        let data: ConfigData = if file.is_file() {
            match std::fs::read_to_string(&file).map_err(Report::from)
                .and_then(|v|serde_json::from_str(&v).map_err(Report::from)) {
                Ok(v) => v,
                Err(e) => {
                    error!("Unable to read config at {:?}: {}",file,e);
                    Default::default()
                }
            }
        } else {
            Default::default()
        };
        let (tx, rx,child) = spawn_audio().expect("Can't start audio controller");
        // TODO: don't use into_owned, avoid copy
        Self {
            path: data.path,
            play_next: Default::default(),
            pause: Default::default(),
            volume_input: Default::default(),
            favorite: Default::default(),
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
            playtime: None,
            total_playtime: None,
            child,
        }
    }
}

impl Drop for PlaybackControl {
    fn drop(&mut self) {
        self.store_state().unwrap();
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
                format!("{:02}:{:02}",minutes,secs_total - (minutes * 60))
            },
        };
        let mut row_controls = Row::new()
        .align_items(Align::Center)
        .spacing(20)
        .push(
            Button::new(&mut self.play_next, Text::new(play_text))
                .on_press(Message::PlayNext),
        )
        .push(
            Button::new(&mut self.pause, Text::new(pause_text)).on_press(Message::Pause),
        );

        if !self.current_file.is_empty() {
            row_controls = row_controls.push(
                Button::new(&mut self.favorite, Text::new(fav_text))
                    .on_press(Message::ToggleFavorite),
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
            .push(
                row_controls,
            )
            .push(
                Text::new(length_text)
                    .size(20)
                    .width(Length::Fill)
                    .horizontal_alignment(HorizontalAlignment::Center),
            )
            .push(
                Text::new(format!("{}% Volume",self.volume))
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
                Text::new("Drop a playlist file to start (.M3U/.PLS/.XSPF/.ASX)")
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
                self.tx.send(PlayerCommand::Pause).expect("Can't send playback command!");
            }
            Message::SliderChanged(v) => {
                self.volume = v;
                self.tx.send(PlayerCommand::Volume(v)).expect("Can't send playback command!");
            }
            Message::Window(iced_native::Event::Window(
                iced_native::window::Event::FileDropped(f),
            )) => {
                match std::fs::read_to_string(&f) {
                    Ok(data) => match playlist_decoder::decode(&data) {
                        Ok(mut playlist) => {
                            if let Some(v) = self.playlists.get_mut(&f) {
                                if v.is_empty() {
                                    v.append(&mut playlist);
                                }
                            } else {
                                playlist.shuffle(&mut thread_rng());
                                self.playlists.insert(f.clone(), playlist);
                            }
                            self.path = f;
                            //return Command::perform(Message::PlayNext);
                        }
                        Err(e) => eprintln!("{}", e),
                    },
                    Err(e) => warn!("Can't open playlist {}", e),
                }
            }
            Message::Tick => {
                //info!("Tick start");
                if let Ok(msg) = self.rx.try_recv() {
                    match msg {
                        PlayerStatus::Playing(f,length) => {
                            self.current_file = f;
                            self.is_paused = false;
                            self.is_favorite = self.data_favorites.contains(&self.current_file);
                            dbg!(length);
                            self.length = length;
                        }
                        PlayerStatus::Ended => {
                            println!("Playback ended");
                            self.play_next();
                            self.current_file = String::new();
                            if let Err(e) = self.store_state() {
                                warn!("Unable to store state! {}", e);
                            }
                        }
                        PlayerStatus::Paused => {
                            self.is_paused = true;
                        }
                    }
                }
            }
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
                    Ok(_) => info!("Favorites written to {}",path),
                    Err(e) => error!("Can't write favorites to {}: {}",path,e),
                }
            }
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
        let window_ticks = iced_native::subscription::events().map(Message::Window);
        Subscription::batch(vec![window_ticks, timer_ticks])
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
    stable_eyre::install().unwrap();
    env_logger::init();
    let mut settings: Settings<()> = Settings::default();
    let mut window_settings = window::Settings::default();
    window_settings.size = (500, 500);
    settings.window = window_settings;
    PlaybackControl::run(settings).unwrap();

    Ok(())
}
