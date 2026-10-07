use crate::command::music_play::MusicPlayArgs;
use crate::command::music_play::ring_buffer_wait::RingBufferWait;
use cpal::SAMPLE_RATE_48K;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use indicatif::MultiProgress;
use std::error;
use std::time::{Duration, SystemTime};
use tracing::{error, info};

pub fn args(args: MusicPlayArgs, mp: Option<MultiProgress>) -> Result<(), Box<dyn error::Error>> {
    const BUF_LEN_U32: u32 = 1024;
    const BUF_LEN: usize = BUF_LEN_U32 as usize;
    const SAMPLE_RATE: u32 = SAMPLE_RATE_48K;
    const SAMPLE_RATE_F64: f64 = SAMPLE_RATE as f64;
    // 只做这一件事：往 cpal 回调里填一段正弦波
    let host = cpal::default_host();
    let Some(device) = host.default_output_device() else {
        panic!("No output device available");
    };
    let config = device.default_output_config()?;
    let channels = config.channels();

    let config = cpal::StreamConfig {
        channels,
        sample_rate: SAMPLE_RATE,
        buffer_size: cpal::BufferSize::Fixed(BUF_LEN_U32), // 先试 512
    };
    // 在你的播放器初始化阶段
    let ring_capacity = SAMPLE_RATE as usize * channels as usize;
    let mut ring_buf = RingBufferWait::new_max_len(ring_capacity).expect("创建缓冲区失败");
    let p = 0.1;

    // producer 交给解码线程
    // consumer 交给 cpal 回调（需要 move 进闭包）
    let stream = {
        let mut ring_buf = ring_buf.clone();
        let r = device.build_output_stream(
            config,
            move |data: &mut [f32], info: &cpal::OutputCallbackInfo| {
                ring_buf.read_exact(data).expect("读取缓冲区失败");
            },
            |err| error!("流错误: {err}"),
            None,
        );
        r
    }?;
    stream.play()?;
    let start_time = SystemTime::now();
    let time = Duration::from_secs(60);
    let kt = f64::from(SAMPLE_RATE) / 1000.0;
    let end_max_time = 10.0;
    let end_max_time_k = end_max_time / time.as_secs_f64();

    let mut buf_vec = vec![0.0; BUF_LEN];
    let mut tk = 0.0;
    let mut max_time = 1.0;
    let mut max_time_kt = max_time * kt;
    let mut zero_time_kt = max_time_kt * 2.0;
    while SystemTime::now()
        .duration_since(start_time)
        .expect("时间转换失败")
        < time
    {
        for i in buf_vec.chunks_mut(channels as usize) {
            let v = if tk < max_time_kt {
                (1.0 * (tk / max_time_kt)) as f32
            } else if tk < zero_time_kt {
                (1.0 - 1.0 * ((tk - max_time_kt) / max_time_kt)) as f32
            } else {
                0.0
            };
            for i in i {
                *i = v;
            }
            tk = (tk + 1.0) % SAMPLE_RATE_F64;
            if tk == 0.0 {
                max_time += end_max_time_k;
                max_time_kt = max_time * kt;
                zero_time_kt = max_time_kt * 2.0;
                info!("up_tk, max_time: {max_time}");
            }
        }
        ring_buf.write_all(&buf_vec).expect("写入缓冲区失败");
    }
    let end_time = SystemTime::now();
    info!("start_time: {start_time:?}, end_time: {end_time:?}");
    drop(ring_buf);
    Ok(())
}
