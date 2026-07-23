use crate::command::{BUF_LEN, PACK_PROGRESS_STYLE_TEMPLATE, create_pb, set_pb_style2, update_pb};
use clap::{Args, Subcommand};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::{fs, io};
use tracing::{error, info, warn};
use water_ball_tool::file_finder::{FileFinder, FileInfo, FileKind};
use water_ball_tool::tools::PathTool;
use water_ball_tool::wb_files_pack::allocator::Allocator;
use water_ball_tool::wb_files_pack::{
    PackFileMetadata, PackFileMetadataRun, PackStructItem, PackStructItemType,
};

#[derive(Args, Debug)]
pub(crate) struct WaterBallFilePackArgs {
    #[command(subcommand)]
    commands: WaterBallFilePackCommands,
}

impl WaterBallFilePackArgs {
    #[cfg(test)]
    pub(crate) fn new(commands: WaterBallFilePackCommands) -> Self {
        Self { commands }
    }
}

#[derive(Subcommand, Debug)]
pub(crate) enum WaterBallFilePackCommands {
    //解包
    #[command(visible_alias = "u")]
    Unpack(WaterBallFilePackCommandsUnpack),
    //打包
    #[command(visible_alias = "p")]
    Pack(WaterBallFilePackCommandsPack),
    //对整个包文件哈希校验
    #[command(visible_alias = "h")]
    HashVerify(WaterBallFilePackCommandsHashVerify),
}
//解包参数
#[derive(Args, Debug)]
pub(crate) struct WaterBallFilePackCommandsUnpack {
    ///水球包包文件路径
    pack_path: String,
    ///输出目录路径
    out_dir: String,
    ///解包前哈希校验
    #[arg(short, long)]
    hash_verify: bool,
}
impl WaterBallFilePackCommandsUnpack {
    #[cfg(test)]
    pub(crate) fn new(pack_path: String, out_dir: String, hash_verify: bool) -> Self {
        Self {
            pack_path,
            out_dir,
            hash_verify,
        }
    }
}

//打包参数
#[derive(Args, Debug)]
pub(crate) struct WaterBallFilePackCommandsPack {
    //打包的文件或目录路径
    in_path: String,
    ///输出的包文件路径，忽略将生成同名包文件
    out_pack_path: Option<String>,
    ///不分离清单
    #[arg(short, long)]
    no_separation: bool,
}
impl WaterBallFilePackCommandsPack {
    #[cfg(test)]
    pub(crate) fn new(in_path: String, out_pack_path: Option<String>, no_separation: bool) -> Self {
        Self {
            in_path,
            out_pack_path,
            no_separation,
        }
    }
}
//哈希校验参数
#[derive(Args, Debug)]
pub(crate) struct WaterBallFilePackCommandsHashVerify {
    ///包文件路径
    pack_path: String,
}
impl WaterBallFilePackCommandsHashVerify {
    #[cfg(test)]
    pub(crate) fn new(pack_path: String) -> Self {
        Self { pack_path }
    }
}
/// 水球包文件路由入口
/// WaterBall pack file router
pub fn wbfp(args: WaterBallFilePackArgs, mp: Option<&MultiProgress>) {
    let args2 = &args.commands;
    match args2 {
        WaterBallFilePackCommands::Unpack(u) => WaterBallFilePackArgsRuning::wbfp_s(u, mp),
        WaterBallFilePackCommands::Pack(p) => {
            let wbfp = WaterBallFilePackArgsRuning;
            wbfp.wbfp_m(p, mp)
        }
        WaterBallFilePackCommands::HashVerify(h) => WaterBallFilePackArgsRuning::wbfp_h(h, mp),
    }
}

struct WaterBallFilePackArgsRuning;

impl WaterBallFilePackArgsRuning {
    /// 打包目录为水球包文件。
    ///
    /// 参数: `<输入目录> <包文件路径> [-f]`
    ///
    /// 使用 `FileFinder` 多线程扫描源目录，将每个文件写入包的虚拟文件系统。
    /// `-f` 标志强制将清单数据嵌入 `.pack` 文件而非分离的 `.wbm` 文件。
    ///
    /// Pack a directory into a WaterBall pack file.
    ///
    /// Args: `<input_dir> <pack_path> [-f]`
    ///
    /// Uses `FileFinder` to multi-threaded scan the source directory, then writes
    /// each file into the pack's virtual filesystem. The `-f` flag forces manifest
    /// data to be embedded in the `.pack` file instead of a separate `.wbm`.
    pub fn wbfp_m(&self, args: &WaterBallFilePackCommandsPack, mp: Option<&MultiProgress>) {
        use std::thread;
        //源目录路径
        let in_dir_path = PathBuf::from(&args.in_path);
        //输出的包文件路径
        if let Some(pack_path) = &args.out_pack_path {
            //分离数据文件
            let separate_manifest = !args.no_separation;

            //进度条
            let ff_pb = create_pb(mp); //搜索进度条
            //包文件进度条
            let wb_pb = create_pb(mp).map(|pb| Arc::new(Mutex::new(pb)));

            info!("开始准备打包");
            info!("创建新包文件并初始化");
            let pack = Allocator::create_new_pack_file(&pack_path, false, separate_manifest)
                .expect("创建包文件错误");
            //逻辑实现=== ===
            //搜索文件===
            info!("开始搜索文件");
            warn!("目前搜索将跳过符号链接");
            //搜索结果队列
            let results = Arc::new(Mutex::new(VecDeque::new()));
            //搜索结束标志
            let ff_end = Arc::new(Mutex::new(false));
            //控制线程继续和等待的控制变量
            let condver = Arc::new(Condvar::new());
            //文件计数
            let file_count = Arc::new(Mutex::new(0));
            let data_len = Arc::new(Mutex::new(0));

            //移动到线程的变量
            //创建专门的线程搜索
            let ff_thread = {
                let in_dir_path = in_dir_path.clone();
                let results = results.clone();
                let ff_end = ff_end.clone();
                let condver = condver.clone();
                let file_count = file_count.clone();
                let data_len = data_len.clone();

                thread::spawn(move || -> io::Result<()> {
                    let ff = FileFinder;
                    let dir_count = Arc::new(Mutex::new(0));
                    //搜索进度
                    let (tx, rx) = mpsc::channel();
                    let (search_stream_handle, stream_results) =
                        ff.search_stream(in_dir_path.as_ref(), true, tx, 2)?;
                    //更新进度线程

                    if let Some(ff_pb) = ff_pb {
                        let file_count = file_count.clone();
                        let dir_count = dir_count.clone();
                        thread::spawn(move || {
                            for (add_file, add_dir) in rx {
                                let mut file_count = file_count.lock().unwrap();
                                let mut dir_count = dir_count.lock().unwrap();

                                *file_count += add_file;
                                *dir_count += add_dir;
                                let all_count = *file_count + *dir_count;
                                ff_pb.set_position(all_count);
                                //10的倍数才更新
                                if all_count.is_multiple_of(10) {
                                    ff_pb.set_message(format!(
                                        "[搜索文件]已发现 {file_count} 文件和 {dir_count} 个目录"
                                    ));
                                }
                            }
                        });
                    }

                    for item in stream_results {
                        //只有文件才会加入队列
                        if let FileKind::File = item.1.file_kind() {
                            *data_len.lock().unwrap() += item.1.length();
                            results.lock().unwrap().push_back(item);
                            condver.notify_one();
                        }
                    }

                    search_stream_handle.join().unwrap()?;
                    info!(
                        "文件搜索已完成: {}个文件，{}个目录",
                        *file_count.lock().unwrap(),
                        *dir_count.lock().unwrap()
                    );
                    *ff_end.lock().unwrap() = true;
                    Ok(())
                })
            };

            //复制操作===
            info!("开始复制数据");
            //设置为包文件具体进度条
            if let Some(pb) = &wb_pb {
                let pb = pb.lock().unwrap();
                pb.set_length(*data_len.lock().unwrap());
                pb.set_style(
                    ProgressStyle::default_bar()
                        .template(PACK_PROGRESS_STYLE_TEMPLATE)
                        .unwrap()
                        .progress_chars("=>-"),
                );
                pb.set_prefix("总进度");
            }

            let thread_count = 8;
            //复制线程
            let wp_thread = {
                let pack = pack.clone();
                let results = results.clone();
                let in_dir_path = in_dir_path.clone();
                let condver = condver.clone();
                let ff_end = ff_end.clone();

                //创建线程进度条
                let mut wb_pbs = Vec::with_capacity(thread_count);
                for _ in 0..thread_count {
                    let pb = create_pb(mp);
                    if let Some(pb) = &pb {
                        pb.set_style(
                            ProgressStyle::default_bar()
                                .template(PACK_PROGRESS_STYLE_TEMPLATE)
                                .unwrap()
                                .progress_chars("=>-"),
                        );
                    }
                    wb_pbs.push(pb);
                }

                thread::spawn(move || {
                    let (tx, rx) = mpsc::channel();
                    let mut thread_handle = Vec::with_capacity(thread_count);
                    //
                    for index in 0..thread_count {
                        let main_pb_tx = tx.clone();
                        let pack_man = pack.clone();
                        let files_list = results.clone();
                        let in_dir_path = in_dir_path.clone();
                        let condver = condver.clone();
                        let pb = wb_pbs.pop().unwrap();
                        let ff_end = ff_end.clone();
                        if let Some(pb) = &pb {
                            pb.set_prefix(format!("[线程{index}] "));
                        }
                        thread_handle.push(thread::spawn(move || {
                            Self::write_pack(
                                pack_man,
                                pb,
                                files_list,
                                in_dir_path,
                                ff_end,
                                condver,
                                main_pb_tx,
                            )
                        }))
                    }

                    drop(tx); //释放用不到的发送器，否则死锁。
                    //计数变量
                    let mut write_file_count = 0;
                    let mut write_len = 0;
                    let mut last_write_len = 0;
                    //更新总进度
                    for (file_count_add, len_add) in rx {
                        write_file_count += file_count_add;
                        write_len += len_add;
                        //更新进度
                        if write_len - last_write_len > 10 * 1024 * 1024
                            && let Some(pb) = &wb_pb
                        {
                            let pb = pb.lock().unwrap();
                            pb.set_length(*data_len.lock().unwrap());
                            pb.set_position(write_len);
                            pb.set_message(format!(
                                "[{write_file_count}/{}个文件]",
                                file_count.lock().unwrap()
                            ));
                            last_write_len = write_len;
                        }
                    }

                    //等待所有工作线程结束
                    for item in thread_handle {
                        item.join().unwrap().unwrap();
                    }
                })
            };
            //等待线程结束
            ff_thread.join().unwrap().unwrap();
            wp_thread.join().unwrap();
            info!("操作已完成,文件保存到{pack_path}");
        }
    }
    fn write_pack(
        mut pack_man: Allocator,
        mut pb: Option<ProgressBar>,
        files_list: Arc<Mutex<VecDeque<(PathBuf, FileInfo)>>>,
        in_dir_path: PathBuf,
        ff_end: Arc<Mutex<bool>>,
        condver: Arc<Condvar>,
        main_pb_tx: Sender<(u64, u64)>,
    ) -> io::Result<()> {
        let mut run_buf = vec![0u8; 1024 * 1024];
        'write_pack: loop {
            let (file_path, file_info) = match files_list.lock().unwrap().pop_front() {
                Some(v) => v,
                None => {
                    //进入挂起
                    loop {
                        //判断搜索是否结束, 确定是否退出线程
                        if *ff_end.lock().unwrap() {
                            break 'write_pack;
                        } else {
                            //设置进度条样式
                            if let Some(pb) = &pb {
                                pb.set_style(
                                    ProgressStyle::default_bar()
                                        .template("{prefix}{spinner:.green} {msg}")
                                        .unwrap(),
                                );
                                pb.set_message("已挂起");
                            }
                            let mut files_list = files_list.lock().unwrap();
                            files_list = condver.wait(files_list).unwrap();
                            let v = {
                                //恢复进度条样式
                                if let Some(pb) = &pb {
                                    pb.set_style(
                                        ProgressStyle::default_bar()
                                            .template(PACK_PROGRESS_STYLE_TEMPLATE)
                                            .unwrap()
                                            .progress_chars("=>-"),
                                    );
                                }

                                files_list.pop_front()
                            };
                            match v {
                                Some(v) => break v,
                                None => continue,
                            }
                        }
                    }
                }
            };
            //写入文件前处理
            //路径处理
            let pack_path = {
                let in_dir_path_vec = PathTool::path_to_string_vec(&in_dir_path);
                let file_path_vec = PathTool::path_to_string_vec(&file_path);
                let mut new_path = PathBuf::new();

                for name in &file_path_vec[in_dir_path_vec.len()..] {
                    new_path = new_path.join(name);
                }
                new_path
            };

            let mut thread_write_len = 0;
            let mut last_thread_write_len = 0;

            Self::copy_file_into_pack(
                &mut pack_man,
                &mut pb,
                &mut run_buf,
                &file_info,
                &file_path,
                &pack_path,
                &main_pb_tx,
                &mut thread_write_len,
                &mut last_thread_write_len,
            );
            let _ = main_pb_tx.send((1, 0)); //更新总进度条的文件数量
        }
        Ok(())
    }

    fn copy_file_into_pack(
        pack_man: &mut Allocator,
        pb: &mut Option<ProgressBar>,
        run_buf: &mut [u8],
        info: &FileInfo,
        this_in_path: &Path,
        this_pack_path: &Path,
        main_pb_tx: &Sender<(u64, u64)>,
        thread_write_len: &mut u64,
        last_thread_write_len: &mut u64,
    ) {
        let mut update_pb = |write_len| {
            *thread_write_len = write_len;
            if *thread_write_len - *last_thread_write_len >= 5 * 1024 * 1024
                && let Some(pb) = pb
            {
                pb.set_length(info.length());
                pb.set_position(write_len);
                pb.set_message(format!(
                    "File path: {}\
                \nPack path: {}",
                    this_in_path.display(),
                    this_pack_path.display()
                ));
                *last_thread_write_len = *thread_write_len;
            }
        };
        //尝试打开文件
        let mut in_file = match File::open(this_in_path) {
            Ok(file) => file,
            Err(err) => {
                match err.kind() {
                    ErrorKind::PermissionDenied => {
                        //权限不足
                        error!(
                            r#"无法打开文件"{}"，权限不足，将跳过，err:{err}"#,
                            this_in_path.display()
                        );
                    }
                    _ => error!(
                        r#"[未处理错误]无法打开文件"{}"，err:{err}"#,
                        this_in_path.display()
                    ),
                }
                return;
            }
        };
        //尝试创建虚拟文件
        let mut out_file =
            match pack_man.create_file(this_pack_path, info.modified_time(), info.length()) {
                Ok(v) => v,
                Err(err) => {
                    error!(
                        r#"无法创建虚拟文件"{}",将跳过, err:{err}"#,
                        this_pack_path.display()
                    );
                    return;
                }
            };
        //写入操作
        let mut write_len = 0;
        while write_len < info.length() {
            //读
            match in_file.read(run_buf) {
                Ok(this_read_len) => {
                    match out_file.write(&run_buf[..this_read_len]) {
                        Ok(this_write_len) => {
                            assert_ne!(
                                this_read_len,
                                0,
                                r#"从文件"{}"读取的大小为0，但于预期不符，文件大小可能不是0"#,
                                this_pack_path.display()
                            );
                            if this_write_len < this_read_len {
                                warn!(
                                    r#"从文件"{}"写入虚拟文件"{}"大小不一致，读：{this_read_len}B，写：{this_write_len}B"#,
                                    this_in_path.display(),
                                    this_pack_path.display()
                                );
                            }
                            write_len += this_write_len as u64;
                            //更新进度
                            update_pb(write_len);
                            let _ = main_pb_tx.send((0, this_write_len as u64)); //更新总进度条
                        }
                        Err(err) => {
                            error!(
                                r#"写入虚拟文件"{}"错误, 将跳过，err:{err}"#,
                                this_pack_path.display()
                            );
                            break;
                        }
                    }
                }
                Err(err) => {
                    error!(
                        r#"读取文件"{}"失败，将跳过，err:{err}"#,
                        this_in_path.display()
                    );
                    break;
                }
            }
        }
    }

    /// 解包水球包文件到磁盘目录。
    ///
    /// 参数: `<包文件路径> <输出目录>`
    ///
    /// 加载包内所有元数据和文件结构，将每个虚拟文件写入输出目录。
    /// 大文件（>512MiB）在日志中单独提示。
    ///
    /// Unpack a WaterBall pack file to a disk directory.
    ///
    /// Args: `<pack_path> <output_dir>`
    ///
    /// Loads all metadata and file structure from the pack, then writes each
    /// virtual file to the output directory. Large files (>512MiB) are noted in logs.
    pub fn wbfp_s(args: &WaterBallFilePackCommandsUnpack, mp: Option<&MultiProgress>) {
        let pack_path = &args.pack_path;
        //输出文件路径
        let out_dir_path = &args.out_dir;
        //进度条
        let pb = create_pb(mp);
        info!("开始准备解包");
        info!("打开包文件");
        let mut pack = Allocator::open_pack_file(pack_path).expect("打开包文件错误");
        //逻辑实现=== ===
        info!("开始复制数据");
        fs::create_dir_all(out_dir_path).expect("无法创建数据路径");
        Self::read_pack(&mut pack, Option::from(&pb), out_dir_path.as_ref()).expect("写入文件错误");
        info!("操作已完成,文件保存到目录{out_dir_path}");
    }

    fn read_pack(
        pack_man: &mut Allocator,
        pb: Option<&ProgressBar>,
        out_dir_path: &Path,
    ) -> io::Result<()> {
        /// 递归从包读取文件 / Recursively read files from pack
        fn read_pack_recursive<'a>(
            pack_man: &mut Allocator,
            mut pb_c: Option<&'a mut (dyn FnMut(u64, u64, String, String) + 'a)>,
            pack_struct_items: &HashMap<String, PackStructItem>,
            out_s_path_buf: &Path,
            pack_s_path_buf: &Path,
            run_buf: &mut [u8],
        ) -> io::Result<Option<&'a mut dyn FnMut(u64, u64, String, String)>> {
            for (name, item) in pack_struct_items {
                let this_out_path = out_s_path_buf.join(name);
                let this_pack_path = pack_s_path_buf.join(name);
                match item.item_type() {
                    PackStructItemType::File { .. } => {
                        if let PackFileMetadataRun::Loaded(metadata) = item.metadata() {
                            WaterBallFilePackArgsRuning::copy_pack_file_to_disk(
                                pack_man,
                                &mut pb_c,
                                run_buf,
                                metadata,
                                &this_out_path,
                                &this_pack_path,
                            );
                        } else {
                            error!("虚拟路径{this_pack_path:?}文件的元数据没有被加载");
                        }
                    }
                    PackStructItemType::Dir { pack_struct, .. } => {
                        if let Some(pack_struct) = pack_struct {
                            //尝试获取结构项
                            //创建目录
                            fs::create_dir_all(&this_out_path)?;
                            //目录仅递归处理
                            pb_c = read_pack_recursive(
                                pack_man,
                                pb_c,
                                pack_struct.items(),
                                &this_out_path,
                                &this_pack_path,
                                run_buf,
                            )?;
                        } else {
                            error!("虚拟路径{this_pack_path:?}目录的结构没有被加载");
                        }
                    }
                }
            }
            Ok(pb_c)
        }

        //
        info!("加载所有结构项和元数据");
        pack_man.load_all_data(false)?;
        info!("加载完成");

        //获取根列表
        let root_struct_list = pack_man.get_root_struct_items()?;
        let mut buf = vec![0; BUF_LEN];
        let mut this_all_write_len = 0;
        let mut this_all_write_file_count = 0;
        let attribute = pack_man.get_manifest_attribute()?;
        let all_file_count = attribute.file_count();
        let data_len = attribute.data_len();
        //设置进度条样式
        set_pb_style2(pb, data_len);
        let mut binding = |add_len, add_file_count, f_path, p_path| {
            this_all_write_len += add_len;
            this_all_write_file_count += add_file_count;
            if let Some(pb) = pb {
                pb.set_position(this_all_write_len);
                let percent = ((this_all_write_len as f64) / (data_len as f64)) * 100.0;
                pb.set_message(format!(
                    "[{percent:>6.2}%][{this_all_write_file_count}/{all_file_count}个文件]\
                \nPack path: {p_path}\
                \nFile path: {f_path}"
                ));
            }
        };
        read_pack_recursive(
            pack_man,
            match pb {
                Some(_) => Some(&mut binding),
                None => None,
            },
            &root_struct_list,
            out_dir_path,
            &PathBuf::new(),
            &mut buf,
        )?;
        Ok(())
    }

    fn copy_pack_file_to_disk(
        pack_man: &mut Allocator,
        pb_c: &mut Option<&mut dyn FnMut(u64, u64, String, String)>,
        run_buf: &mut [u8],
        metadata: &PackFileMetadata,
        this_out_path: &PathBuf,
        this_pack_path: &PathBuf,
    ) {
        //更新进度
        let mut lase_up_pb_c_write_len = 0;
        if metadata.len() > 1024 * 1024 * 512 {
            info!(
                r#"正在从包文件虚拟路径"{}"复制大文件到"{}"，大小:{}[{}]"#,
                this_pack_path.display(),
                this_out_path.display(),
                water_ball_tool::tools::bytes_len_to_string(metadata.len()),
                metadata.len()
            );
        }
        if let Some(pb_c) = pb_c {
            pb_c(
                0,
                1,
                this_pack_path.display().to_string(),
                this_out_path.display().to_string(),
            );
        }
        //尝试打开虚拟文件
        let mut in_file = match pack_man.get_file_wr(this_pack_path, false) {
            Ok(file) => file,
            Err(err) => {
                error!("无法打开虚拟文件{this_pack_path:?}，将跳过，err:{err}");
                return;
            }
        };
        //尝试创建文件
        let mut out_file = match File::create(this_out_path) {
            Ok(file_wr) => file_wr,
            Err(err) => {
                error!("无法创建文件{this_out_path:?},将跳过，err：{err}");
                return;
            }
        };
        //分配空间
        if let Err(err) = out_file.set_len(metadata.len()) {
            warn!("无法对输出文件{this_out_path:?}进行预分配空间，将继续, err:{err}");
        }
        //写入操作
        let mut write_len = 0;
        while write_len < metadata.len() {
            //读
            match in_file.read(run_buf) {
                Ok(this_read_len) => {
                    match out_file.write(&run_buf[..this_read_len]) {
                        Ok(this_write_len) => {
                            if this_read_len == 0 {
                                warn!("虚拟文件{this_pack_path:?}读取的大小为0, 将跳过。");
                                break;
                            }
                            if this_write_len != this_read_len {
                                warn!(
                                    "虚拟文件{this_pack_path:?}写入文件{this_out_path:?}大小不一致，读：{this_read_len}，写：{this_write_len}"
                                );
                            }
                            write_len += this_write_len as u64;
                            //更新进度
                            update_pb(
                                pb_c,
                                this_pack_path,
                                this_out_path,
                                &mut lase_up_pb_c_write_len,
                                &mut write_len,
                            );
                        }
                        Err(err) => {
                            error!("写入文件{this_pack_path:?}错误, 将跳过，err:{err}");
                            break;
                        }
                    }
                }
                Err(err) => {
                    error!("读取虚拟文件{this_pack_path:?}失败，将跳过，err:{err}");
                    break;
                }
            }
        }
        //不论是否写入成功都对齐进度条
        if let Some(pb_c) = pb_c {
            pb_c(
                metadata.len() - write_len,
                0,
                this_pack_path.display().to_string(),
                this_out_path.display().to_string(),
            );
        }
    }
    //包文件哈希校验
    fn wbfp_h(args: &WaterBallFilePackCommandsHashVerify, mp: Option<&MultiProgress>) {
        //包文件路径
        let pack_path = &args.pack_path;

        //进度条
        let pb = create_pb(mp);
        info!("开始准备哈希校验");
        info!("打开包文件");
        let mut pack = Allocator::open_pack_file(pack_path).expect("打开包文件错误");
        //逻辑实现=== ===
        info!("开始哈希校验");
        Self::verify_hash(&mut pack, pb.as_ref()).unwrap();
        info!("操作已完成，没有警告（WARN）或错误（ERROR）说明全部通过。");
    }

    fn verify_hash(pack: &mut Allocator, pb: Option<&ProgressBar>) -> io::Result<()> {
        info!("加载所有结构项和元数据");
        pack.load_all_data(false)?;
        info!("加载完成");
        let root_name_list = pack.get_root_struct_item_name_list()?;
        let mut this_all_write_len = 0;
        let mut this_all_write_file_count = 0;
        let attribute = pack.get_manifest_attribute()?;
        let all_file_count = attribute.file_count();
        let data_len = attribute.data_len();

        //设置进度条样式
        set_pb_style2(pb, data_len);
        let mut binding = |add_len, add_file_count, p_path| {
            this_all_write_len += add_len;
            this_all_write_file_count += add_file_count;
            if let Some(pb) = pb {
                pb.set_position(this_all_write_len);
                let percent = ((this_all_write_len as f64) / (data_len as f64)) * 100.0;
                pb.set_message(format!(
                    "[{percent:>6.2}%][{this_all_write_file_count}/{all_file_count}个文件] \
                \nPack path: {p_path}"
                ));
            }
        };

        for root_name in root_name_list {
            let _ = Self::verify_hash_inner(
                pack,
                root_name.as_ref(),
                if pb.is_some() {
                    Some(&mut binding)
                } else {
                    None
                },
            );
        }
        Ok(())
    }

    fn verify_hash_inner<'a>(
        pack: &mut Allocator,
        path: &Path,
        mut pb_c: Option<&'a mut (dyn FnMut(u64, u64, String) + 'a)>,
    ) -> io::Result<Option<&'a mut dyn FnMut(u64, u64, String)>> {
        let item = match pack.get_pack_struct_item(path) {
            Ok(v) => v,
            Err(err) => {
                error!(r#"无法获取虚拟路径"{}"结构项, err:{err}"#, path.display());
                return Ok(pb_c);
            }
        };
        match item.item_type() {
            PackStructItemType::Dir { .. } => {
                let items_name = match pack.get_struct_item_name_list(path) {
                    Ok(v) => v,
                    Err(err) => {
                        error!(
                            r#"无法获取虚拟路径"{}"的结构项名称, err:{err}"#,
                            path.display()
                        );
                        return Ok(pb_c);
                    }
                };
                let mut pb_c = pb_c;
                for name in items_name {
                    pb_c = Self::verify_hash_inner(pack, &path.join(name), pb_c)?;
                }
                Ok(pb_c)
            }
            PackStructItemType::File { .. } => {
                let mut rw = match pack.get_file_wr(path, false) {
                    Ok(v) => v,
                    Err(err) => {
                        error!("无法获取包文件读写器，err: {err}");
                        return Ok(pb_c);
                    }
                };
                match rw.verify_hash() {
                    Ok(false) => {
                        warn!(r#"虚拟文件"{}"哈希验证失败"#, path.display());
                    }
                    Err(err) => warn!(
                        r#"虚拟文件"{}"哈希验证发生错误, err: {err:?}"#,
                        path.display()
                    ),
                    _ => (),
                }
                if let Some(pb) = &mut pb_c {
                    pb(rw.get_len()?, 1, path.display().to_string());
                }
                Ok(pb_c)
            }
        }
    }
}
