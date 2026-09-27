use rodio::{MixerDeviceSink, Player};

use crate::command::music_play::data::PlayList;

struct Running {
    play_list: PlayList,
    handle: MixerDeviceSink,
    player: Player
}

impl Running {
    fn new() -> Self {
        let handle = rodio::DeviceSinkBuilder::open_default_sink().unwrap();
        todo!()
    }
}
