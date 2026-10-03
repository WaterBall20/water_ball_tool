use crate::command::music_play::MusicPlayArgs;
use crate::command::music_play::ring_buffer_wait::RingBufferWait;
use cpal::SAMPLE_RATE_48K;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use indicatif::MultiProgress;
use std::error;
use std::time::{Duration, SystemTime};
use tracing::{error, info};

pub fn args(args: MusicPlayArgs, mp: Option<MultiProgress>) -> Result<(), Box<dyn error::Error>> {
    const RING_CAPACITY: usize = 4096 * 2; // 4096 帧 * 2 声道（f32 样本数）
    const BUF_LEN_U32: u32 = 1024;
    const BUF_LEN: usize = BUF_LEN_U32 as usize;
    const SAMPLE_RATE: u32 = SAMPLE_RATE_48K;
    // 只做这一件事：往 cpal 回调里填一段正弦波
    let host = cpal::default_host();
    let Some(device) = host.default_output_device() else {
        panic!("No output device available");
    };

    let config = cpal::StreamConfig {
        channels: 2,
        sample_rate: SAMPLE_RATE,
        buffer_size: cpal::BufferSize::Fixed(BUF_LEN_U32), // 先试 512
    };
    // 在你的播放器初始化阶段

    let mut ring_buf = RingBufferWait::new_max_len(RING_CAPACITY).expect("创建缓冲区失败");
    let p = 0.1;

    // producer 交给解码线程
    // consumer 交给 cpal 回调（需要 move 进闭包）
    let stream = {
        let mut ring_buf = ring_buf.clone();
        let r = device.build_output_stream(
            config,
            move |data: &mut [f32], info: &cpal::OutputCallbackInfo| {
                data.fill(0.0);
                let read_len = ring_buf.read(data).expect("读取缓冲区失败");
                // 先填一段 440 Hz 正弦波验证连通性
                // 用 info.timestamp() 检查实际回调周期
                info!(
                    "从缓冲区读取， data len: {}, read len : {read_len}, info: {info:?}",
                    data.len()
                );
            },
            |err| error!("流错误: {err}"),
            None,
        );
        r
    }?;
    stream.play()?;
    let start_time = SystemTime::now();
    let time = Duration::from_secs(10);
    let mut buf_vec = vec![0.0; BUF_LEN];
    while SystemTime::now()
        .duration_since(start_time)
        .expect("时间转换失败")
        < time
    {
        for v in &mut buf_vec {
            *v = rand::random_range(-p..p);
        }
        let len = ring_buf.write(&buf_vec).expect("写入缓冲区失败");
        info!("写入缓冲区, len: {len}");
    }
    let end_time = SystemTime::now();
    info!("start_time: {start_time:?}, end_time: {end_time:?}");
    drop(ring_buf);
    Ok(())
}
