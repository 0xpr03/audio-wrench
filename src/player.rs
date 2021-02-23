use std::{
    sync::mpsc::{channel, Receiver, Sender, TryRecvError},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use rodio::{OutputStreamHandle, Sink, Source};
use url::Url;

use crate::prelude::*;

pub struct Player {
    sink: Option<Sink>,
    last_file: String,
    ended: bool,
    length: Option<Duration>,
    play_start: Option<Instant>,
    pause_start: Option<Instant>,
    pause_time: Duration,
    stream_handle: OutputStreamHandle,
    rx: Receiver<PlayerCommand>,
    state_tx: Sender<PlayerStatus>,
}

impl Player {
    pub fn new() -> Result<(
        Sender<PlayerCommand>,
        Receiver<PlayerStatus>,
        JoinHandle<()>,
    )> {
        let (tx, rx) = channel::<PlayerCommand>();
        let (state_tx, state_rx) = channel::<PlayerStatus>();

        let child = thread::Builder::new()
            .name("audio controller".to_string())
            .spawn(move || {
                // can't initialize audio on same thread due to "OleInitialize failed! Result was: `RPC_E_CHANGED_MODE"
                let (_stream, stream_handle) = rodio::OutputStream::try_default().unwrap();
                let mut data = Self {
                    sink: None,
                    last_file: Default::default(),
                    ended: false,
                    length: None,
                    play_start: None,
                    pause_start: None,
                    pause_time: Default::default(),
                    stream_handle,
                    state_tx,
                    rx,
                };
                data.run();
            })?;
        Ok((tx, state_rx, child))
    }
    /// Handle player activity
    /// Returns true
    fn run(&mut self) {
        loop {
            match self.rx.try_recv() {
                Ok(msg) => {
                    trace!("Player command: {:?}", msg);
                    match msg {
                        PlayerCommand::Volume(v) => {
                            if let Some(ref sink) = self.sink {
                                sink.set_volume(calc_volume(v));
                            }
                        }
                        PlayerCommand::Play(origin_path, volume) => self.play(origin_path, volume),
                        PlayerCommand::Pause => self.pause(),
                    }
                }
                Err(TryRecvError::Empty) => {
                    if self.sink.as_ref().map_or(true, |v| v.empty()) && !self.ended {
                        self.state_tx
                            .send(PlayerStatus::Ended)
                            .expect("Can't send playback status!");
                        self.ended = true;
                    } else {
                        let playtime = match self.play_start {
                            Some(play_start) => match self.pause_start {
                                Some(pause_start) => Some(
                                    play_start.elapsed() - self.pause_time - pause_start.elapsed(),
                                ),
                                None => Some(play_start.elapsed() - self.pause_time),
                            },
                            None => None,
                        };
                        self.state_tx
                            .send(PlayerStatus::Playtime(playtime))
                            .expect("Can't send playback status!");
                        thread::sleep(Duration::from_millis(150));
                    }
                }
                Err(TryRecvError::Disconnected) => {
                    break;
                }
            }
        }
    }

    fn play(&mut self, origin_path: String, volume: u8) {
        self.ended = false;
        if let Some(ref v) = self.sink {
            v.stop();
        }
        let path = match Url::parse(&origin_path) {
            Ok(v) => match v.to_file_path() {
                Ok(v) => v,
                Err(_) => {
                    warn!("Can't play URLs, skipping");
                    return;
                }
            },
            Err(_e) => origin_path.clone().into(),
        };
        match std::fs::File::open(&path) {
            Ok(file) => {
                debug!("Starting playback");
                self.last_file = path.to_string_lossy().into_owned();
                let input = match rodio::Decoder::new(file) {
                    Ok(v) => v,
                    Err(e) => {
                        warn!("Can't play {:?} unsupported format?: {:?}", origin_path, e);

                        self.state_tx
                            .send(PlayerStatus::InvalidFile(origin_path.clone()))
                            .expect("Can't send playback status!");
                        return;
                    }
                };
                self.length = input.total_duration();
                debug!("size_hint {:?}", input.size_hint());
                let new_sink =
                    Sink::try_new(&self.stream_handle).expect("Can't open new playback-sink!");
                new_sink.set_volume(calc_volume(volume));
                new_sink.append(input);
                self.sink = Some(new_sink);
                self.state_tx
                    .send(PlayerStatus::Playing(self.last_file.clone(), self.length))
                    .expect("Can't send playback status!");
                self.play_start = Some(Instant::now());
                self.pause_time = Default::default();
                self.pause_start = None;
            }
            Err(e) => warn!("{:?} {}", path, e),
        }
    }

    fn pause(&mut self) {
        self.ended = false;
        if let Some(ref mut sink) = self.sink {
            if sink.is_paused() {
                if let Some(time) = self.pause_start {
                    self.pause_time = self.pause_time + time.elapsed();
                    self.pause_start = None;
                }
                sink.play();
                self.state_tx
                    .send(PlayerStatus::Playing(self.last_file.clone(), self.length))
                    .expect("Can't send playback status!");
            } else {
                self.pause_start = Some(Instant::now());
                sink.pause();
                self.state_tx
                    .send(PlayerStatus::Paused)
                    .expect("Can't send playback status!");
            }
        }
    }
}

fn calc_volume(v: u8) -> f32 {
    (v as f32) / 100.0
}

#[derive(Debug)]
pub enum PlayerCommand {
    Volume(u8),
    Play(String, u8),
    Pause,
}

#[derive(Debug, PartialEq)]
pub enum PlayerStatus {
    Playing(String, Option<Duration>),
    Ended,
    InvalidFile(String),
    Paused,
    Playtime(Option<Duration>),
}
