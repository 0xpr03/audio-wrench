//#![windows_subsystem = "windows"]

use dirs::data_local_dir;
use iced::{Align, Application, Element, Settings, Subscription, executor, window};
use log::{info, warn};
use stable_eyre::eyre::{eyre, Report, WrapErr};
use url::Url;

type Result<T> = std::result::Result<T, Report>;
use std::{borrow::Cow, collections::HashMap, fs::File, io::Write, path::PathBuf, sync::mpsc::{Receiver, Sender, TryRecvError}, time::Duration};
use std::io::BufReader;
use rodio::{OutputStream, OutputStreamHandle, Sink, Source};
use std::thread;
use serde::Deserialize;
use serde::Serialize;
use rand::prelude::*;
use iced_native::{Button, Column, Command, HorizontalAlignment, Length, Row, Slider, Text, button, renderer, slider};

use std::env;
use std::sync::{Arc, Mutex,mpsc::channel};

#[derive(Serialize, Deserialize, Default)]
struct ConfigData<'a> {
    playlists: Cow<'a, HashMap<PathBuf,Vec<String>>>,
    volume: u8,
    path: PathBuf,
    current_playlist: Cow<'a, String>,
}

struct PlaybackControl {
    path: PathBuf,
    play_next: button::State,
    pause: button::State,
    volume_input: slider::State,
    volume: u8,
    tx: Sender<PlayerCommand>,
    rx: Receiver<PlayerStatus>,
    current_playlist: String,
    current_file: String,
    playlists: HashMap<PathBuf,Vec<String>>,
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
                self.tx.send(PlayerCommand::Play(v[0].clone(),self.volume)).unwrap();
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
    Tick
}

enum PlayerCommand {
    Volume(u8),
    Play(String,u8),
    Pause,
}


enum PlayerStatus {
    Playing(String),
    Ended,
    Paused,
}

fn spawn_audio() -> (Sender<PlayerCommand>,Receiver<PlayerStatus>) {
    let (tx,rx) = channel::<PlayerCommand>();
    let (update_tx,state_rx) = channel::<PlayerStatus>();
    thread::spawn(move|| {
        let (_stream, stream_handle) = rodio::OutputStream::try_default().unwrap();
        let mut sink: Option<Sink> = None;
        let mut last_volume: f32 = 0.5;
        let mut last_file: String = Default::default();
        let mut ended = false;
        loop {
            match rx.try_recv(){
                Ok(msg) => match msg {
                    PlayerCommand::Volume(v) => {
                        last_volume = (v as f32) / 100.0;
                        if let Some(ref sink) = sink {
                            sink.set_volume(last_volume);
                        }
                    }
                    PlayerCommand::Play(path,volume) => {
                        ended = false;
                        last_volume = (volume as f32) / 100.0;
                        if let Some(ref v) = sink {
                            v.stop();
                        }
                        let path = match Url::parse(&path) {
                            Ok(v) => match v.to_file_path() {
                                Ok(v) => v,
                                Err(_) => {warn!("Can't playback URLs"); continue},
                            }
                            Err(_e) => path.into(),
                        };
                        match std::fs::File::open(&path) {
                            Ok(file) => {
                                println!("Starting playback");
                                last_file = path.to_string_lossy().into_owned();
                                let input = rodio::Decoder::new(file).unwrap();
                                let new_sink = Sink::try_new(&stream_handle).unwrap();
                                new_sink.set_volume(last_volume);
                                new_sink.append(input);
                                sink = Some(new_sink);
                                update_tx.send(PlayerStatus::Playing(last_file.clone())).unwrap();
                            },
                            Err(e) => warn!("{:?} {}",path,e),
                        }
                    }
                    PlayerCommand::Pause => {
                        ended = false;
                        if let Some(ref mut sink) = sink {
                            if sink.is_paused() {
                                sink.play();
                                update_tx.send(PlayerStatus::Playing(last_file.clone())).unwrap();
                            } else {
                                sink.pause();
                                update_tx.send(PlayerStatus::Paused).unwrap();
                            }
                            
                        }
                    }
                },
                Err(TryRecvError::Empty) => {
                    if sink.as_ref().map_or(true, |v|v.empty()) && !ended {
                        update_tx.send(PlayerStatus::Ended).unwrap();
                        ended = true;
                    } else {
                        thread::sleep(Duration::from_millis(100));
                    }
                },
                Err(TryRecvError::Disconnected) => {
                    break;
                },
            }
        }
    });
    (tx,state_rx)
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
            let data = std::fs::read_to_string(&file).unwrap();
            serde_json::from_str(&data).unwrap()
        } else {
            Default::default()
        };
        let(tx,rx) = spawn_audio();
        Self {
            path: data.path,
            play_next: Default::default(),
            pause: Default::default(),
            volume_input: Default::default(),
            volume: data.volume,
            tx,
            rx,
            playlists: data.playlists.into_owned(),
            current_playlist: data.current_playlist.into_owned(),
            current_file: Default::default(),
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
        Column::new().max_width(800).spacing(20).align_items(Align::Center)
        .push(Text::new(&self.current_playlist.to_string()).size(20).width(Length::Fill)
            .horizontal_alignment(HorizontalAlignment::Center))
        .push(Text::new(&self.current_file.to_string()).size(20).width(Length::Fill)
            .horizontal_alignment(HorizontalAlignment::Center))
        .push(Row::new().align_items(Align::Center).spacing(20)
            .push(Button::new(&mut self.play_next,Text::new("Play/Next"))
                .on_press(Message::PlayNext))
            .push(Button::new(&mut self.pause,Text::new("Pause"))
                .on_press(Message::Pause))
        )
        .push(Text::new(&self.volume.to_string()).size(20).width(Length::Fill)
        .horizontal_alignment(HorizontalAlignment::Center))
        .push(Slider::new(&mut self.volume_input,
            0..=100,
            self.volume,
            Message::SliderChanged))
        .padding(20)
        .push(Text::new("Drop a playlist file to start").size(20).width(Length::Fill)
            .horizontal_alignment(HorizontalAlignment::Center))
        .into()

    }

    fn update(&mut self, message: Message) -> Command<Self::Message> {
        match message {
            Message::PlayNext => {
                self.play_next();
            }
            Message::Pause => {
                self.tx.send(PlayerCommand::Pause).unwrap();
            }
            Message::SliderChanged(v) => {
                self.volume = v;
                self.tx.send(PlayerCommand::Volume(v)).unwrap();
            }
            Message::Window(iced_native::Event::Window(iced_native::window::Event::FileDropped(f))) => {
                let data = std::fs::read_to_string(&f).unwrap();
                match playlist_decoder::decode(&data) {
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
                    },
                    Err(e) => eprintln!("{}",e),
                }
            }
            Message::Tick => {
                //info!("Tick start");
                if let Ok(msg) = self.rx.try_recv() {
                    match msg {
                        PlayerStatus::Playing(f) => {
                            self.current_file = f;
                        }
                        PlayerStatus::Ended => {
                            println!("Playback ended");
                            self.play_next();
                            if let Err(e) = self.store_state() {
                                warn!("Unable to store state! {}",e);
                            }
                        }
                        PlayerStatus::Paused => {

                        }
                    }
                }
            },
            Message::Window(_) => (),
        }
        Command::none()
    }

    fn new(_flags: ()) -> (PlaybackControl, Command<Message>) {
        (
            PlaybackControl::default(),
            Command::none(),
        )
    }

    fn title(&self) -> String {
        String::from("Audio Wrench")
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        let timer_ticks = iced::time::every(Duration::from_millis(100)).map(|_|Message::Tick);
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
    window_settings.size = (500,400);
    settings.window = window_settings;
    PlaybackControl::run(settings).unwrap();

    Ok(())
}

