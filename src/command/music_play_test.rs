use crate::command::music_play::MusicPlayArgs;
use crate::command::music_play::ring_buffer_wait::RingBufferWait;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use indicatif::MultiProgress;
use std::error;
use tracing::{debug, error, info};

pub fn args(args: MusicPlayArgs, mp: Option<MultiProgress>) -> Result<(), Box<dyn error::Error>> {
    // 只做这一件事：往 cpal 回调里填一段正弦波
    let host = cpal::default_host();
    let Some(device) = host.default_output_device() else {
        panic!("No output device available");
    };

    let config = cpal::StreamConfig {
        channels: 2,
        sample_rate: 48000,
        buffer_size: cpal::BufferSize::Fixed(512), // 先试 512
    };
    // 在你的播放器初始化阶段
    const RING_CAPACITY: usize = 4096 * 2; // 4096 帧 * 2 声道（f32 样本数）
    let mut ring_buf = RingBufferWait::new_max_len(RING_CAPACITY);
    let p = 0.1;

    // producer 交给解码线程
    // consumer 交给 cpal 回调（需要 move 进闭包）
    let stream = {
        let mut ring_buf = ring_buf.clone();
        let r = device.build_output_stream(
            config,
            move |data: &mut [f32], info: &cpal::OutputCallbackInfo| {
                ring_buf.read(data).expect("读取缓冲区失败");
                // 先填一段 440 Hz 正弦波验证连通性
                // 用 info.timestamp() 检查实际回调周期
                info!("info: {info:?}");
            },
            |err| error!("流错误: {err}"),
            None,
        );
        r
    }?;
    stream.play()?;
    const BUF_LEN: usize = 512;
    let mut buf_vec = vec![0.0; BUF_LEN];
    for _ in 0..1024 {
        for j in 0..BUF_LEN {
            buf_vec[j] = rand::random_range(-p..p);
        }
        ring_buf.write(&buf_vec).expect("写入缓冲区失败");
    }
    drop(ring_buf);
    Ok(())
}
