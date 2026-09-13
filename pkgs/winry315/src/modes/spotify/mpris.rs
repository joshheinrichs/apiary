//! Driving Spotify over MPRIS.
//!
//! The bus name is owned by bubbled-spotify's `xdg-dbus-proxy`, not Spotify
//! itself; the proxy's `--dbus-own` grant is what makes this reachable.
//! Tighten that policy and the mode goes dead in a way that looks like a pad
//! bug.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::ops::Deref;
use zbus::blocking::Connection;
use zbus::zvariant::{OwnedValue, Value};

const MPRIS_DEST: &str = "org.mpris.MediaPlayer2.spotify";
const MPRIS_PATH: &str = "/org/mpris/MediaPlayer2";
const MPRIS_PLAYER: &str = "org.mpris.MediaPlayer2.Player";
const DBUS_PROPS: &str = "org.freedesktop.DBus.Properties";

pub struct Player(Connection);

impl Player {
    pub fn connect() -> Result<Player> {
        Ok(Player(Connection::session().context("session bus")?))
    }

    fn call<B>(&self, method: &str, body: &B) -> Result<()>
    where
        B: zbus::export::serde::Serialize + zbus::zvariant::DynamicType,
    {
        self.0
            .call_method(Some(MPRIS_DEST), MPRIS_PATH, Some(MPRIS_PLAYER), method, body)
            .with_context(|| format!("MPRIS {method}"))?;
        Ok(())
    }

    /// Owned rather than borrowed: the reply is a temporary, and a `Value`
    /// would still be pointing into it by the time the caller looked.
    fn get(&self, property: &str) -> Result<OwnedValue> {
        let reply = self
            .0
            .call_method(
                Some(MPRIS_DEST),
                MPRIS_PATH,
                Some(DBUS_PROPS),
                "Get",
                &(MPRIS_PLAYER, property),
            )
            .with_context(|| format!("reading MPRIS {property}"))?;
        reply.body().deserialize().context("unexpected reply")
    }

    pub fn play_pause(&self) -> Result<()> {
        self.call("PlayPause", &())
    }

    pub fn next(&self) -> Result<()> {
        self.call("Next", &())
    }

    pub fn previous(&self) -> Result<()> {
        self.call("Previous", &())
    }

    pub fn seek(&self, micros: i64) -> Result<()> {
        self.call("Seek", &(micros,))
    }

    pub fn volume(&self) -> Result<f64> {
        f64::try_from(self.get("Volume")?).context("MPRIS Volume was not a double")
    }

    pub fn set_volume(&self, level: f64) -> Result<()> {
        self.0
            .call_method(
                Some(MPRIS_DEST),
                MPRIS_PATH,
                Some(DBUS_PROPS),
                "Set",
                &(MPRIS_PLAYER, "Volume", Value::F64(level).try_to_owned()?),
            )
            .context("setting MPRIS Volume")?;
        Ok(())
    }

    /// The current track's art URL, if the player is advertising one.
    /// Spotify's is a content hash, so it is stable per album -- comparing URLs
    /// is enough to avoid refetching for every track on a record.
    pub fn art_url(&self) -> Result<Option<String>> {
        // Properties.Get answers with a variant wrapping the dict, so unwrap
        // one layer before the a{sv} is reachable. Deserialising straight to a
        // map fails with `got 'v', expected 'a{sv}'`.
        let metadata = HashMap::<String, OwnedValue>::try_from(self.get("Metadata")?)
            .context("Metadata was not a dict")?;
        match metadata.get("mpris:artUrl").map(Deref::deref) {
            Some(Value::Str(url)) => Ok(Some(url.to_string())),
            _ => Ok(None),
        }
    }

    /// Is Spotify playing? Levels can only be trusted while it is: a capture
    /// stream whose target is missing falls back to the default source, so with
    /// Spotify closed the pad would otherwise dance to the microphone.
    pub fn playing(&self) -> bool {
        matches!(self.get("PlaybackStatus").as_deref(), Ok(Value::Str(s)) if s == "Playing")
    }
}
