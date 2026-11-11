// SPDX-FileCopyrightText: 2026 Chris Palmer <chris@red-oxide.org>
// SPDX-License-Identifier: GPL-3.0-only

//! Playing podcasts with GStreamer, in-process: audio through `playbin`,
//! video through `iced_video_player` (also `playbin`), whose frames the
//! article pane draws.

use std::{
    fmt,
    sync::{Arc, Mutex},
    time::Duration,
};

use gstreamer::{self as gst, prelude::*};
use iced_video_player::Video;

/// What the player is doing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayerState {
    /// Seconds into the media.
    pub position: f64,
    pub duration: Option<f64>,
    pub paused: bool,
    pub speed: f64,
    /// Played to the end.
    pub ended: bool,
}

/// A loaded episode. Dropping it stops playback.
pub trait Player: Send {
    /// `None` if playback failed.
    fn state(&mut self) -> Option<PlayerState>;
    fn set_paused(&mut self, paused: bool);
    /// Seeks to `seconds` from the start.
    fn seek(&mut self, seconds: f64);
    fn set_speed(&mut self, speed: f64);
    /// The frames to draw, for video.
    fn video(&self) -> Option<&Video> {
        None
    }
}

/// Loads `url` from `start` seconds, as video or audio only. Blocks while
/// the stream starts, so it runs off the UI thread.
pub type Loader = fn(&str, u32, bool) -> Result<Box<dyn Player>, String>;

/// A loaded player, passed in a `Message` (which must be `Clone` and
/// `Debug`); the reader takes it out once.
#[derive(Clone)]
pub struct Loaded(Arc<Mutex<Option<Box<dyn Player>>>>);

impl Loaded {
    pub fn new(player: Box<dyn Player>) -> Self {
        Self(Arc::new(Mutex::new(Some(player))))
    }

    pub fn take(&self) -> Option<Box<dyn Player>> {
        self.0.lock().ok()?.take()
    }
}

impl fmt::Debug for Loaded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Loaded")
    }
}

// Not measured: needs GStreamer's plugins, a sound device and a stream to
// play, none of which tests can count on; the reader drives a fake.
#[cfg(not(tarpaulin_include))]
pub fn load(url: &str, start: u32, video: bool) -> Result<Box<dyn Player>, String> {
    if video {
        let url = url::Url::parse(url).map_err(|e| e.to_string())?;
        let mut video = Video::new(&url).map_err(|e| format!("could not play the video: {e}"))?;
        if start > 0 {
            video
                .seek(Duration::from_secs(start.into()), false)
                .map_err(|e| e.to_string())?;
        }
        Ok(Box::new(VideoPlayer(video)))
    } else {
        Ok(Box::new(
            Audio::new(url, start).map_err(|e| format!("could not play: {e}"))?,
        ))
    }
}

/// Audio only: `playbin` with its video thrown away, and `scaletempo` so
/// faster playback doesn't raise the pitch.
struct Audio {
    playbin: gst::Element,
    speed: f64,
    ended: bool,
}

#[cfg(not(tarpaulin_include))]
impl Audio {
    fn new(url: &str, start: u32) -> Result<Self, gst::glib::Error> {
        gst::init()?;
        let playbin = gst::ElementFactory::make("playbin")
            .property("uri", url)
            .build()
            .map_err(|e| gst::glib::Error::new(gst::CoreError::MissingPlugin, &e.to_string()))?;
        if let Ok(sink) = gst::ElementFactory::make("fakesink").build() {
            playbin.set_property("video-sink", &sink);
        }
        if let Ok(filter) = gst::ElementFactory::make("scaletempo").build() {
            playbin.set_property("audio-filter", &filter);
        }
        let failed = |e: &dyn fmt::Display| {
            gst::glib::Error::new(gst::CoreError::StateChange, &e.to_string())
        };
        playbin
            .set_state(gst::State::Paused)
            .map_err(|e| failed(&e))?;
        // Wait until it can seek: the stream has started.
        playbin
            .state(gst::ClockTime::from_seconds(20))
            .0
            .map_err(|e| failed(&e))?;
        if start > 0 {
            let _ = playbin.seek_simple(
                gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT,
                gst::ClockTime::from_seconds(start.into()),
            );
        }
        playbin
            .set_state(gst::State::Playing)
            .map_err(|e| failed(&e))?;
        Ok(Self {
            playbin,
            speed: 1.0,
            ended: false,
        })
    }

    fn seek_at(&self, seconds: f64, speed: f64) {
        let at = gst::ClockTime::from_nseconds((seconds.max(0.0) * 1e9) as u64);
        let _ = self.playbin.seek(
            speed,
            gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
            gst::SeekType::Set,
            at,
            gst::SeekType::None,
            gst::ClockTime::NONE,
        );
    }
}

#[cfg(not(tarpaulin_include))]
impl Player for Audio {
    fn state(&mut self) -> Option<PlayerState> {
        let bus = self.playbin.bus()?;
        while let Some(message) = bus.pop() {
            match message.view() {
                gst::MessageView::Eos(_) => self.ended = true,
                gst::MessageView::Error(_) => return None,
                _ => {}
            }
        }
        let seconds = |t: gst::ClockTime| t.nseconds() as f64 / 1e9;
        Some(PlayerState {
            position: self
                .playbin
                .query_position::<gst::ClockTime>()
                .map_or(0.0, seconds),
            duration: self.playbin.query_duration::<gst::ClockTime>().map(seconds),
            paused: self.playbin.current_state() != gst::State::Playing,
            speed: self.speed,
            ended: self.ended,
        })
    }

    fn set_paused(&mut self, paused: bool) {
        let state = if paused {
            gst::State::Paused
        } else {
            gst::State::Playing
        };
        let _ = self.playbin.set_state(state);
    }

    fn seek(&mut self, seconds: f64) {
        self.ended = false;
        self.seek_at(seconds, self.speed);
    }

    fn set_speed(&mut self, speed: f64) {
        let position = self
            .playbin
            .query_position::<gst::ClockTime>()
            .map_or(0.0, |t| t.nseconds() as f64 / 1e9);
        self.speed = speed;
        self.seek_at(position, speed);
    }
}

#[cfg(not(tarpaulin_include))]
impl Drop for Audio {
    fn drop(&mut self) {
        let _ = self.playbin.set_state(gst::State::Null);
    }
}

/// Video, drawn in the article pane.
struct VideoPlayer(Video);

#[cfg(not(tarpaulin_include))]
impl Player for VideoPlayer {
    fn state(&mut self) -> Option<PlayerState> {
        let duration = self.0.duration().as_secs_f64();
        Some(PlayerState {
            position: self.0.position().as_secs_f64(),
            duration: (duration > 0.0).then_some(duration),
            paused: self.0.paused(),
            speed: self.0.speed(),
            ended: self.0.eos(),
        })
    }

    fn set_paused(&mut self, paused: bool) {
        self.0.set_paused(paused);
    }

    fn seek(&mut self, seconds: f64) {
        let _ = self
            .0
            .seek(Duration::from_secs_f64(seconds.max(0.0)), false);
    }

    fn set_speed(&mut self, speed: f64) {
        let _ = self.0.set_speed(speed);
    }

    fn video(&self) -> Option<&Video> {
        Some(&self.0)
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A player tests control: `state` is what it reports, `calls` what it
    /// was told.
    #[derive(Clone, Default)]
    pub struct Fake {
        pub state: Arc<Mutex<Option<PlayerState>>>,
        pub calls: Arc<Mutex<Vec<String>>>,
        /// The copy given to the reader, whose drop is "stop".
        boxed: bool,
    }

    impl Fake {
        /// The copy to hand to the reader.
        pub fn boxed(&self) -> Box<dyn Player> {
            Box::new(Self {
                state: self.state.clone(),
                calls: self.calls.clone(),
                boxed: true,
            })
        }

        pub fn at(&self, position: f64, paused: bool, ended: bool) {
            *self.state.lock().unwrap() = Some(PlayerState {
                position,
                duration: Some(1000.0),
                paused,
                speed: 1.0,
                ended,
            });
        }

        pub fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }

        fn call(&self, call: String) {
            self.calls.lock().unwrap().push(call);
        }
    }

    impl Player for Fake {
        fn state(&mut self) -> Option<PlayerState> {
            *self.state.lock().unwrap()
        }
        fn set_paused(&mut self, paused: bool) {
            self.call(format!("paused {paused}"));
            if let Some(s) = self.state.lock().unwrap().as_mut() {
                s.paused = paused;
            }
        }
        fn seek(&mut self, seconds: f64) {
            self.call(format!("seek {seconds}"));
            if let Some(s) = self.state.lock().unwrap().as_mut() {
                s.position = seconds;
            }
        }
        fn set_speed(&mut self, speed: f64) {
            self.call(format!("speed {speed}"));
            if let Some(s) = self.state.lock().unwrap().as_mut() {
                s.speed = speed;
            }
        }
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            if self.boxed {
                self.call("stop".into());
            }
        }
    }

    #[test]
    fn loaded_players_are_taken_once() {
        let fake = Fake::default();
        let loaded = Loaded::new(fake.boxed());
        assert_eq!(format!("{loaded:?}"), "Loaded");
        let copy = loaded.clone();
        assert!(copy.take().is_some());
        assert!(loaded.take().is_none());
        assert!(Fake::default().video().is_none());
        assert_eq!(fake.calls(), ["stop"]);
    }
}
