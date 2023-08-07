use std::collections::HashMap;
use std::thread::sleep;
use std::time::Duration;
use std::sync::Arc;
use dbus::blocking::Connection;
use dbus::message::Message;
use dbus::tree::{MethodErr, MethodResult};
use dbus::channel::MatchingReceiver;
use unicode_width::UnicodeWidthStr;

const MESSAGE_DISPLAY_LEN: usize = 20;
const FONT_INDEX: u32 = 1;
const UPDATE_DELAY: u64 = 300;
const CONTROL_CHARS: [&str; 4] = ["", "", "", ""];

const DISPLAY_PLAYER_PREFIX: [(&str, &str); 3] = [
    ("spotify", ""),
    ("firefox", ""),
    ("default", ""),
];

const METADATA_FIELDS: [&str; 2] = ["xesam:title", "xesam:artist"];
const METADATA_SEPARATOR: char = '-';
const HIDE_OUTPUT: bool = false;

struct PlayerInfo {
    name: String,
    player: dbus::blocking::Proxy<&'static str>,
}

impl PlayerInfo {
    fn new(name: String, player: dbus::blocking::Proxy<&'static str>) -> Self {
        PlayerInfo { name, player }
    }
}

struct PolybarNowPlaying {
    connection: Connection,
    players: Vec<PlayerInfo>,
    current_player: usize,
    display_prefix: String,
    display_suffix: String,
    status_paused: bool,
}

impl PolybarNowPlaying {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let connection = Connection::new_session()?;
        let players = PolybarNowPlaying::get_players(&connection)?;
        let mut instance = PolybarNowPlaying {
            connection,
            players,
            current_player: 0,
            display_prefix: String::new(),
            display_suffix: String::new(),
            status_paused: false,
        };
        instance.update_players()?;
        Ok(instance)
    }

    fn get_players(connection: &Connection) -> Result<Vec<PlayerInfo>, Box<dyn std::error::Error>> {
        let names = connection.list_names()?;
        let mut players = Vec::new();

        for name in names {
            if name.starts_with("org.mpris.MediaPlayer2.") {
                let proxy = connection.with_proxy(&name, "/org/mpris/MediaPlayer2", Duration::from_millis(5000));
                players.push(PlayerInfo::new(name.to_string(), proxy));
            }
        }

        Ok(players)
    }

    fn update_players(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.players = PolybarNowPlaying::get_players(&self.connection)?;
        if self.current_player >= self.players.len() {
            self.current_player = 0;
        }
        Ok(())
    }

    fn get_status(&self, player: &PlayerInfo) -> Result<String, Box<dyn std::error::Error>> {
        let status: String = player.player.method_call("org.freedesktop.DBus.Properties", "Get",
            ("org.mpris.MediaPlayer2.Player", "PlaybackStatus"))?.read1()?;
        Ok(status)
    }

    fn get_metadata(&self, player: &PlayerInfo) -> Result<HashMap<String, String>, Box<dyn std::error::Error>> {
        let metadata: HashMap<String, String> = player.player.method_call("org.freedesktop.DBus.Properties", "Get",
            ("org.mpris.MediaPlayer2.Player", "Metadata"))?.read1()?;
        Ok(metadata)
    }

    fn update_prefix_suffix(&mut self, player_name: &str, status: &str) {
        let player_option = if player_name.is_empty() { "".to_string() } else { format!("-p {}", player_name) };
        let prev_button = format!("%%{{A:playerctl {} previous :}}{}%%{{A}}", player_option, CONTROL_CHARS[0]);
        let play_button = format!("%%{{A:playerctl {} play :}}{}%%{{A}}", player_option, CONTROL_CHARS[1]);
        let pause_button = format!("%%{{A:playerctl {} pause :}}{}%%{{A}}", player_option, CONTROL_CHARS[2]);
        let next_button = format!("%%{{A:playerctl {} next :}}{}%%{{A}}", player_option, CONTROL_CHARS[3]);

        let mut suffix = format!("| {}", prev_button);
        if status == "Playing" {
            suffix += &format!(" {}", pause_button);
            self.status_paused = false;
        } else {
            suffix += &format!(" {}", play_button);
            self.status_paused = true;
        }
        suffix += &format!(" {}", next_button);
        self.display_suffix = suffix;

        for (key, value) in &DISPLAY_PLAYER_PREFIX {
            if player_name.to_lowercase().contains(key) {
                self.display_prefix = value.to_string();
                break;
            }
        }
        if self.display_prefix.is_empty() {
            self.display_prefix = DISPLAY_PLAYER_PREFIX.last().unwrap().1.to_string();
        }
    }

    fn update_message(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if self.players.is_empty() {
            self.display_prefix = String::new();
            self.display_suffix = String::new();
            self.display_suffix = "No player available".to_string();
            self.update_prefix_suffix("", "");
        } else {
            let player_info = &self.players[self.current_player];
            let player_name = &player_info.name;
            let status = self.get_status(player_info)?;
            let metadata = self.get_metadata(player_info)?;

            let mut metadata_string_list = Vec::new();
            for field in &METADATA_FIELDS {
                if let Some(result) = metadata.get(*field) {
                    metadata_string_list.push(result.to_string());
                } else {
                    metadata_string_list.push(format!("No {}", field.split(":").last().unwrap()));
                }
            }
            let metadata_string = metadata_string_list.join(&format!(" {} ", METADATA_SEPARATOR));
            let metadata_display_len = self.visual_length(&metadata_string);
            if metadata_display_len > MESSAGE_DISPLAY_LEN {
                self.update_prefix_suffix(player_name, &status);
                let metadata_string = format!(" {} ", METADATA_SEPARATOR) + &metadata_string + " |";
                self.display_suffix = self.make_visual_length(&metadata_string, MESSAGE_DISPLAY_LEN);
            } else {
                self.display_suffix = String::new();
                self.update_prefix_suffix(player_name, &status);
            }
        }

        if HIDE_OUTPUT && self.players.is_empty() {
            println!("");
        } else {
            self.scroll();
            let display_text = format!("{} %{{T{}}}{}%{{T-}}{}", self.display_prefix, FONT_INDEX, self.display_text(), self.display_suffix);
            print!("{}", display_text);
            std::io::stdout().flush()?;
        }

        Ok(())
    }

    fn scroll(&mut self) {
        if !self.status_paused {
            if self.display_text().width() > MESSAGE_DISPLAY_LEN {
                self.display_text = self.display_text()[1..].to_string() + &self.display_text()[0..1];
            } else if self.display_text().width() < MESSAGE_DISPLAY_LEN {
                self.display_text += &" ".repeat(MESSAGE_DISPLAY_LEN - self.display_text().width());
            }
        }
    }

    fn visual_length(&self, text: &str) -> usize {
        text.width()
    }

    fn make_visual_length(&self, text: &str, visual_desired_length: usize) -> String {
        let mut visual_length = 0;
        let mut altered_text = String::new();

        for ch in text.chars() {
            let width = if ch.width() == Some(2) { 2 } else { 1 };
            if visual_length + width <= visual_desired_length {
                visual_length += width;
                altered_text.push(ch);
            } else {
                break;
            }
        }

        if visual_length == visual_desired_length + 1 {
            altered_text.pop();
            altered_text.push(' ');
        } else if visual_length < visual_desired_length {
            altered_text.push_str(&" ".repeat(visual_desired_length - visual_length));
        }

        altered_text
    }

    fn display_text(&self) -> &str {
        &self.display_text
    }

    fn run(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        loop {
            sleep(Duration::from_millis(UPDATE_DELAY));
            self.update_players()?;
            self.update_message()?;
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut polybar_now_playing = PolybarNowPlaying::new()?;
    polybar_now_playing.run()?;
    Ok(())
}
