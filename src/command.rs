/*
创建时间:26/02/24 80:40
*/
use crate::command::ff::FileFinderArgs;
use crate::command::wbfp::WaterBallFilePackArgs;
use clap::{Parser, Subcommand};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::path::Path;
use std::time::Duration;

mod ff;
#[cfg(test)]
mod test;
mod wbfp;

#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
#[command(propagate_version = true)]
pub(crate) struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Ff(FileFinderArgs),
    Wbfp(WaterBallFilePackArgs),
}

static BUF_LEN: usize = 1024 * 1024;

const PROGRESS_STYLE_TEMPLATE: &str =
    "{spinner:.green} [{elapsed_precise}({eta})] [{bar:40.cyan/blue}] {msg:>7}";

const PACK_PROGRESS_STYLE_TEMPLATE: &str = "{prefix}{spinner:.green} [{bar:40.cyan/blue}] [{elapsed_precise}(ETA:{eta})] {percent:>6.2}% {bytes:>11}/{total_bytes:>11} \n {msg}";

//创建进度实例
fn create_pb(mp: Option<&MultiProgress>) -> Option<ProgressBar> {
    if let Some(mp) = mp {
        let pb = mp.add(ProgressBar::new_spinner());
        pb.enable_steady_tick(Duration::from_millis(100)); // 让转标自己动起来
        Some(pb)
    } else {
        None
    }
}
//设置进度样式：显示进度条
fn set_pb_style2(pb: Option<&ProgressBar>, data_len: u64) {
    if let Some(pb) = pb {
        pb.set_length(data_len);
        pb.set_style(
            ProgressStyle::default_bar()
                .template(PROGRESS_STYLE_TEMPLATE)
                .unwrap()
                .progress_chars("=>-"),
        );
        pb.set_message("0.00%");
    }
}

fn update_pb(
    pb_c: &mut Option<&mut dyn FnMut(u64, u64, String, String)>,
    path1: &Path,
    path2: &Path,
    lase_up_pb_c_write_len: &mut u64,
    write_len: &mut u64,
) {
    if let Some(pb_c) = pb_c {
        let l_len = *write_len - *lase_up_pb_c_write_len;
        if l_len > 10 * (BUF_LEN as u64) {
            pb_c(
                l_len,
                0,
                path1.display().to_string(),
                path2.display().to_string(),
            );
            *lase_up_pb_c_write_len = *write_len;
        }
    }
}

pub(crate) fn cli(cli: Cli, mp: Option<&MultiProgress>) {
    match cli.command {
        Commands::Ff(ff) => ff::ff(ff, mp),
        Commands::Wbfp(wbfp) => wbfp::wbfp(wbfp, mp),
    }
}
