// SPDX-FileCopyrightText: 2026 Chris Palmer <chris@red-oxide.org>
// SPDX-License-Identifier: GPL-3.0-only

//! The system's media controls: MPRIS on Linux (so media keys, headphones
//! and Birdland's bar can drive playback) and Now Playing on macOS.

use std::{sync::Mutex, time::Duration};

use iced::futures::channel::mpsc::{UnboundedSender, unbounded};
use souvlaki::{MediaControlEvent, SeekDirection};

/// What the controls ask for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MediaKey {
    Play,
    Pause,
    Toggle,
    /// Seconds to skip, back if negative.
    SeekBy(f64),
    /// Seconds from the start.
    SeekTo(f64),
    Stop,
}

/// How far "next" and "previous" (headphone buttons) skip, in seconds:
/// podcast players skip rather than change episodes.
pub const FORWARD: f64 = 30.0;
pub const BACK: f64 = 10.0;

/// The keys pressed, if they are ones the player handles.
pub fn key(event: MediaControlEvent) -> Option<MediaKey> {
    let signed = |direction, seconds: f64| match direction {
        SeekDirection::Forward => seconds,
        SeekDirection::Backward => -seconds,
    };
    Some(match event {
        MediaControlEvent::Play => MediaKey::Play,
        MediaControlEvent::Pause => MediaKey::Pause,
        MediaControlEvent::Toggle => MediaKey::Toggle,
        MediaControlEvent::Next => MediaKey::SeekBy(FORWARD),
        MediaControlEvent::Previous => MediaKey::SeekBy(-BACK),
        MediaControlEvent::Seek(d) => MediaKey::SeekBy(signed(d, FORWARD)),
        MediaControlEvent::SeekBy(d, by) => MediaKey::SeekBy(signed(d, by.as_secs_f64())),
        MediaControlEvent::SetPosition(p) => MediaKey::SeekTo(p.0.as_secs_f64()),
        MediaControlEvent::Stop | MediaControlEvent::Quit => MediaKey::Stop,
        _ => return None,
    })
}

/// What the controls show.
pub trait Remote {
    fn now_playing(&mut self, title: &str, feed: &str, image: Option<&str>, duration: Option<f64>);
    /// `None` when stopped; otherwise whether paused, and the position.
    fn playback(&mut self, state: Option<(bool, f64)>);
}

/// Where keys go: the subscription's channel, once it runs.
static KEYS: Mutex<Option<UnboundedSender<MediaKey>>> = Mutex::new(None);

fn send(event: MediaControlEvent) {
    if let Some(key) = key(event)
        && let Ok(keys) = KEYS.lock()
        && let Some(keys) = keys.as_ref()
    {
        let _ = keys.unbounded_send(key);
    }
}

/// The keys pressed, as a stream for `Subscription::run`.
pub fn keys() -> impl iced::futures::Stream<Item = MediaKey> {
    let (tx, rx) = unbounded();
    if let Ok(mut keys) = KEYS.lock() {
        *keys = Some(tx);
    }
    rx
}

struct Controls(souvlaki::MediaControls);

/// Connects to the system's media controls, if it has any (no D-Bus
/// session, say, and there are none).
// Not measured: needs a D-Bus session (or macOS); `key` and `keys`, which
// carry what it hears, are tested.
#[cfg(not(tarpaulin_include))]
pub fn start() -> Option<Box<dyn Remote>> {
    let config = souvlaki::PlatformConfig {
        display_name: "pressbox",
        dbus_name: "pressbox",
        hwnd: None,
    };
    let mut controls = souvlaki::MediaControls::new(config).ok()?;
    controls.attach(send).ok()?;
    Some(Box::new(Controls(controls)))
}

#[cfg(not(tarpaulin_include))]
impl Remote for Controls {
    fn now_playing(&mut self, title: &str, feed: &str, image: Option<&str>, duration: Option<f64>) {
        let _ = self.0.set_metadata(souvlaki::MediaMetadata {
            title: Some(title),
            artist: Some(feed),
            album: Some(feed),
            cover_url: image,
            duration: duration.map(Duration::from_secs_f64),
        });
    }

    fn playback(&mut self, state: Option<(bool, f64)>) {
        let progress = |at: f64| {
            Some(souvlaki::MediaPosition(Duration::from_secs_f64(
                at.max(0.0),
            )))
        };
        let _ = self.0.set_playback(match state {
            None => souvlaki::MediaPlayback::Stopped,
            Some((true, at)) => souvlaki::MediaPlayback::Paused {
                progress: progress(at),
            },
            Some((false, at)) => souvlaki::MediaPlayback::Playing {
                progress: progress(at),
            },
        });
    }
}

#[cfg(test)]
mod tests {
    use iced::futures::StreamExt;
    use souvlaki::MediaPosition;

    use super::*;

    #[test]
    fn maps_media_keys() {
        use MediaControlEvent as E;
        let cases = [
            (E::Play, Some(MediaKey::Play)),
            (E::Pause, Some(MediaKey::Pause)),
            (E::Toggle, Some(MediaKey::Toggle)),
            (E::Next, Some(MediaKey::SeekBy(FORWARD))),
            (E::Previous, Some(MediaKey::SeekBy(-BACK))),
            (
                E::Seek(SeekDirection::Backward),
                Some(MediaKey::SeekBy(-FORWARD)),
            ),
            (
                E::SeekBy(SeekDirection::Forward, Duration::from_secs(5)),
                Some(MediaKey::SeekBy(5.0)),
            ),
            (
                E::SetPosition(MediaPosition(Duration::from_secs(90))),
                Some(MediaKey::SeekTo(90.0)),
            ),
            (E::Stop, Some(MediaKey::Stop)),
            (E::Quit, Some(MediaKey::Stop)),
            (E::Raise, None),
        ];
        for (event, expected) in cases {
            assert_eq!(key(event.clone()), expected, "{event:?}");
        }
    }

    #[tokio::test]
    async fn keys_reach_the_stream() {
        // Nothing listening yet: dropped.
        *KEYS.lock().unwrap() = None;
        send(MediaControlEvent::Play);
        let mut stream = Box::pin(keys());
        send(MediaControlEvent::Raise);
        send(MediaControlEvent::Toggle);
        assert_eq!(stream.next().await, Some(MediaKey::Toggle));
    }
}
