// TODO: if camera not available, launch alternate UI indicating such and
// waiting for it to become available?

// TODO: add quit app button to UI.

// TODO: UI automatically reconnect to app after app restart.

#![cfg_attr(feature = "backtrace", feature(backtrace))]

#[macro_use]
extern crate serde_derive;

#[macro_use]
extern crate log;

#[cfg(feature = "backend_aravis")]
use ci2_aravis as backend;
#[cfg(feature = "backend_dc1394")]
use ci2_dc1394 as backend;
#[cfg(feature = "backend_flycap2")]
use ci2_flycap2 as backend;
#[cfg(feature = "backend_pyloncxx")]
extern crate ci2_pyloncxx as backend;

#[cfg(feature = "fiducial")]
use ads_apriltag as apriltag;

use http_video_streaming as video_streaming;
use machine_vision_formats as formats;

#[cfg(feature = "flydratrax")]
use nalgebra as na;

#[allow(unused_imports)]
use std::convert::TryInto;

#[cfg(feature = "fiducial")]
use libflate::finish::AutoFinishUnchecked;
#[cfg(feature = "fiducial")]
use libflate::gzip::Encoder;

use futures::{channel::mpsc, sink::SinkExt, stream::StreamExt};

use hyper_tls::HttpsConnector;
#[allow(unused_imports)]
use preferences::{AppInfo, Preferences};

use ci2::{Camera, CameraInfo, CameraModule};
use ci2_async::AsyncCamera;
use fmf::FMFWriter;

use async_change_tracker::ChangeTracker;
use basic_frame::{match_all_dynamic_fmts, DynamicFrame};
use formats::PixFmt;
use timestamped_frame::ExtraTimeData;

use bui_backend::highlevel::{create_bui_app_inner, BuiAppInner, ConnectionEventType};
use bui_backend::lowlevel::EventChunkSender;
use bui_backend::AccessControl;
use bui_backend_types::{CallbackDataAndSession, ConnectionKey, SessionKey};

#[cfg(feature = "flydratrax")]
use http_video_streaming_types::DrawableShape;
use http_video_streaming_types::StrokeStyle;

use video_streaming::{AnnotatedFrame, FirehoseCallback};

use std::path::Path;

#[cfg(feature = "image_tracker")]
use ci2_remote_control::CsvSaveConfig;
use ci2_remote_control::{CamArg, MkvRecordingConfig, RecordingFrameRate};
use flydra_types::{
    BuiServerInfo, CamHttpServerInfo, MainbrainBuiLocation, RawCamName, RealtimePointsDestAddr,
    RosCamName,
};

#[cfg(feature = "image_tracker")]
use image_tracker::{FlyTracker, ImPtDetectCfg, UfmfState};

use strand_cam_csv_config_types::CameraCfgFview2_0_26;
#[cfg(feature = "image_tracker")]
use strand_cam_csv_config_types::{FullCfgFview2_0_26, SaveCfgFview2_0_25};

#[cfg(feature = "fiducial")]
use strand_cam_storetype::ApriltagState;
use strand_cam_storetype::{CallbackType, ImOpsState, RangedValue, StoreType, ToCamtrigDevice};
#[cfg(feature = "flydratrax")]
use strand_cam_storetype::{KalmanTrackingConfig, LedProgramConfig};

#[cfg(feature = "flydratrax")]
use flydra_types::{FlydraFloatTimestampLocal, HostClock, SyncFno, Triggerbox};

#[cfg(feature = "flydratrax")]
use strand_cam_pseudo_cal::PseudoCameraCalibrationData;

use rust_cam_bui_types::RecordingPath;

use parking_lot::RwLock;
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::sync::Arc;

pub const DEBUG_ADDR_DEFAULT: &'static str = "127.0.0.1:8877";

pub const APP_INFO: AppInfo = AppInfo {
    name: "strand-cam",
    author: "AndrewStraw",
};

use crossbeam_ok::CrossbeamOk;
#[cfg(feature = "flydratrax")]
use flydra2::{CoordProcessor, CoordProcessorControl, MyFloat, StreamItem};

#[cfg(feature = "imtrack-absdiff")]
pub use im_pt_detect_config::default_absdiff as default_im_pt_detect;
#[cfg(feature = "imtrack-dark-circle")]
pub use im_pt_detect_config::default_dark_circle as default_im_pt_detect;

include!(concat!(env!("OUT_DIR"), "/frontend.rs")); // Despite slash, this does work on Windows.

#[cfg(feature = "flydratrax")]
const KALMAN_TRACKING_PREFS_KEY: &'static str = "kalman-tracking";

#[cfg(feature = "flydratrax")]
const LED_PROGRAM_PREFS_KEY: &'static str = "led-config";

#[cfg(feature = "flydratrax")]
mod flydratrax_handle_msg;

mod post_trigger_buffer;

#[cfg(feature = "with_camtrig")]
const CAMTRIG_HEARTBEAT_INTERVAL_MSEC: u64 = 5000;

pub type Result<M> = std::result::Result<M, StrandCamError>;

#[derive(Debug, thiserror::Error)]
pub enum StrandCamError {
    // #[error("other error")]
    // OtherError,
    #[error("string error: {0}")]
    StringError(String),
    #[error("no cameras found")]
    NoCamerasFound,
    #[error("ConvertImageError: {0}")]
    ConvertImageError(
        #[from]
        #[cfg_attr(feature = "backtrace", backtrace)]
        convert_image::Error,
    ),
    #[error("FMF error: {0}")]
    FMFError(
        #[from]
        #[cfg_attr(feature = "backtrace", backtrace)]
        fmf::FMFError,
    ),
    #[error("UFMF error: {0}")]
    UFMFError(
        #[from]
        #[cfg_attr(feature = "backtrace", backtrace)]
        ufmf::UFMFError,
    ),
    #[error("io error: {source}")]
    IoError {
        #[from]
        source: std::io::Error,
        #[cfg(feature = "backtrace")]
        backtrace: std::backtrace::Backtrace,
    },
    #[error("try send error")]
    TrySendError,
    #[error("BUI backend error: {0}")]
    BuiBackendError(#[from] bui_backend::Error),
    #[error("ci2 error: {0}")]
    Ci2Error(
        #[from]
        #[cfg_attr(feature = "backtrace", backtrace)]
        ci2::Error,
    ),
    #[error("plugin disconnected")]
    PluginDisconnected,
    #[error("video streaming error")]
    VideoStreamingError(#[from] video_streaming::Error),
    #[error(
        "The --jwt-secret argument must be passed or the JWT_SECRET environment \
                  variable must be set."
    )]
    JwtError,
    #[cfg(feature = "flydratrax")]
    #[error("MVG error: {0}")]
    MvgError(
        #[from]
        #[cfg_attr(feature = "backtrace", backtrace)]
        mvg::MvgError,
    ),
    #[error("{0}")]
    MkvWriterError(
        #[from]
        #[cfg_attr(feature = "backtrace", backtrace)]
        mkv_writer::Error,
    ),
    #[error("{0}")]
    AddrParseError(#[from] std::net::AddrParseError),
    #[error("background movie writer error: {0}")]
    BgMovieWriterError(
        #[from]
        #[cfg_attr(feature = "backtrace", backtrace)]
        bg_movie_writer::Error,
    ),
    #[error("Braid update image listener disconnected")]
    BraidUpdateImageListenerDisconnected,
    #[error("{0}")]
    NvEncError(
        #[from]
        #[cfg_attr(feature = "backtrace", backtrace)]
        nvenc::NvEncError,
    ),
    #[cfg(feature = "flydratrax")]
    #[error("flydra2 error: {0}")]
    Flydra2Error(
        #[from]
        #[cfg_attr(feature = "backtrace", backtrace)]
        flydra2::Error,
    ),
    #[cfg(feature = "flydratrax")]
    #[error("futures mpsc send error: {0}")]
    FuturesChannelMpscSend(#[from] futures::channel::mpsc::SendError),
    #[cfg(feature = "fiducial")]
    #[error("{0}")]
    CsvError(#[from] csv::Error),
    #[error("thread done")]
    ThreadDone,

    #[cfg(feature = "with_camtrig")]
    #[error("{0}")]
    SerialportError(#[from] serialport::Error),
}

pub struct CloseAppOnThreadExit {
    file: &'static str,
    line: u32,
    thread_handle: std::thread::Thread,
    sender: Option<mpsc::Sender<CamArg>>,
}

impl CloseAppOnThreadExit {
    pub fn new(sender: mpsc::Sender<CamArg>, file: &'static str, line: u32) -> Self {
        let thread_handle = std::thread::current();
        Self {
            sender: Some(sender),
            file,
            line,
            thread_handle,
        }
    }

    fn maybe_err(self, result: anyhow::Result<()>) {
        match result {
            Ok(()) => self.success(),
            Err(e) => {
                display_err(
                    e,
                    self.file,
                    self.line,
                    self.thread_handle.name(),
                    self.thread_handle.id(),
                );
                // The drop handler will close everything.
            }
        }
    }

    #[cfg(any(feature = "with_camtrig", feature = "plugin-process-frame"))]
    fn check<T, E>(&self, result: std::result::Result<T, E>) -> T
    where
        E: std::convert::Into<anyhow::Error>,
    {
        match result {
            Ok(v) => v,
            Err(e) => self.fail(e.into()),
        }
    }

    #[cfg(any(feature = "with_camtrig", feature = "plugin-process-frame"))]
    fn fail(&self, e: anyhow::Error) -> ! {
        display_err(
            e,
            self.file,
            self.line,
            self.thread_handle.name(),
            self.thread_handle.id(),
        );
        panic!(
            "panicing thread {:?} due to error",
            self.thread_handle.name()
        );
    }

    fn success(mut self) {
        self.sender.take();
    }
}

fn display_err(
    err: anyhow::Error,
    file: &str,
    line: u32,
    thread_name: Option<&str>,
    thread_id: std::thread::ThreadId,
) {
    eprintln!(
        "Error {}:{} ({:?} Thread name {:?}): {}",
        file, line, thread_id, thread_name, err
    );
    eprintln!("Alternate view of error:",);
    eprintln!("{:#?}", err,);
    eprintln!("Debug view of error:",);
    eprintln!("{:?}", err,);
}

impl Drop for CloseAppOnThreadExit {
    fn drop(&mut self) {
        if let Some(mut sender) = self.sender.take() {
            debug!(
                "*** dropping in thread {:?} {}:{}",
                self.thread_handle.name(),
                self.file,
                self.line
            );
            match futures::executor::block_on(sender.send(CamArg::DoQuit)) {
                Ok(()) => {}
                Err(e) => {
                    error!("failed sending quit command: {}", e);
                    std::process::exit(1);
                }
            }
        }
    }
}

pub(crate) enum Msg {
    StartMkv((String, MkvRecordingConfig)),
    StopMkv,
    StartFMF((String, RecordingFrameRate)),
    StopFMF,
    #[cfg(feature = "image_tracker")]
    StartUFMF(String),
    #[cfg(feature = "image_tracker")]
    StopUFMF,
    #[cfg(feature = "image_tracker")]
    SetTracking(bool),
    PostTriggerStartMkv((String, MkvRecordingConfig)),
    SetPostTriggerBufferSize(usize),
    Mframe(DynamicFrame),
    #[cfg(feature = "image_tracker")]
    SetIsSavingObjDetectionCsv(CsvSaveConfig),
    #[cfg(feature = "image_tracker")]
    SetExpConfig(ImPtDetectCfg),
    Store(Arc<RwLock<ChangeTracker<StoreType>>>),
    #[cfg(feature = "image_tracker")]
    TakeCurrentImageAsBackground,
    #[cfg(feature = "image_tracker")]
    ClearBackground(f32),
    SetFrameOffset(u64),
    SetClockModel(Option<rust_cam_bui_types::ClockModel>),
    QuitFrameProcessThread,
    StartAprilTagRec(String),
    StopAprilTagRec,
}

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub enum FrameProcessingErrorState {
    NotifyAll,
    IgnoreUntil(chrono::DateTime<chrono::Utc>),
    IgnoreAll,
}

impl Default for FrameProcessingErrorState {
    fn default() -> Self {
        FrameProcessingErrorState::NotifyAll
    }
}

#[cfg(feature = "image_tracker")]
#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub enum Tracker {
    NoTracker,
    BackgroundSubtraction(ImPtDetectCfg),
}

/// calculates a framerate every n frames
pub struct FpsCalc<T: chrono::TimeZone> {
    prev: Option<(usize, chrono::DateTime<T>)>,
    frames_to_average: usize,
}

impl<T: chrono::TimeZone> FpsCalc<T> {
    /// create a new FpsCalc instance
    pub fn new(frames_to_average: usize) -> Self {
        Self {
            prev: None,
            frames_to_average,
        }
    }
    /// return a newly computed fps value whenever available.
    pub fn update(&mut self, fno: usize, stamp: chrono::DateTime<T>) -> Option<f64> {
        let mut reset_previous = true;
        let mut result = None;
        if let Some((prev_frame, ref prev_stamp)) = self.prev {
            let n_frames = fno - prev_frame;
            if n_frames < self.frames_to_average {
                reset_previous = false;
            } else {
                let dur_nsec = stamp
                    .clone()
                    .signed_duration_since(prev_stamp.clone())
                    .num_nanoseconds();
                if let Some(nsec) = dur_nsec {
                    result = Some(n_frames as f64 / nsec as f64 * 1.0e9);
                }
            }
        }
        if reset_previous {
            self.prev = Some((fno, stamp.clone()));
        }
        result
    }
}

struct FmfWriteInfo<T>
where
    T: std::io::Write + std::io::Seek,
{
    writer: FMFWriter<T>,
    recording_framerate: RecordingFrameRate,
    last_saved_stamp: Option<std::time::Instant>,
}

impl<T> FmfWriteInfo<T>
where
    T: std::io::Write + std::io::Seek,
{
    fn new(writer: FMFWriter<T>, recording_framerate: RecordingFrameRate) -> Self {
        Self {
            writer,
            recording_framerate,
            last_saved_stamp: None,
        }
    }
}

#[cfg(feature = "fiducial")]
#[derive(Serialize, Deserialize, Debug, Clone)]
struct DetectionSerializer {
    frame: usize,
    time_microseconds: i64,
    id: i32,
    hamming: i32,
    decision_margin: f32,
    h00: f64,
    h01: f64,
    h02: f64,
    h10: f64,
    h11: f64,
    h12: f64,
    h20: f64,
    h21: f64,
    // no h22 because it is always 1.0
    family: String,
}

#[cfg(feature = "fiducial")]
fn my_round(a: f32) -> f32 {
    let b = (a * 10.0).round() as i64;
    b as f32 / 10.0
}

#[cfg(feature = "fiducial")]
fn to_serializer(
    orig: &apriltag::Detection,
    frame: usize,
    time_microseconds: i64,
) -> DetectionSerializer {
    let h = orig.h();
    // We are not going to save h22, so (in debug builds) let's check it meets
    // our expectations.
    debug_assert!((h[8] - 1.0).abs() < 1e-16);
    DetectionSerializer {
        frame,
        time_microseconds,
        id: orig.id(),
        hamming: orig.hamming(),
        decision_margin: my_round(orig.decision_margin()),
        h00: h[0],
        h01: h[1],
        h02: h[2],
        h10: h[3],
        h11: h[4],
        h12: h[5],
        h20: h[6],
        h21: h[7],
        family: orig.family_type().to_str().to_string(),
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct AprilConfig {
    created_at: chrono::DateTime<chrono::Local>,
    camera_name: String,
}

#[cfg(feature = "fiducial")]
struct AprilTagWriter {
    wtr: csv::Writer<Box<dyn std::io::Write>>,
    t0: chrono::DateTime<chrono::Utc>,
}

#[cfg(feature = "fiducial")]
impl AprilTagWriter {
    fn new(template: String, camera_name: &str) -> Result<Self> {
        let now: chrono::DateTime<chrono::Utc> = chrono::Utc::now();
        let local = now.with_timezone(&chrono::Local);
        let fname = local.format(&template).to_string();

        let fd = std::fs::File::create(&fname)?;
        let mut fd: Box<dyn std::io::Write> = Box::new(AutoFinishUnchecked::new(Encoder::new(fd)?));

        let april_config = AprilConfig {
            created_at: local,
            camera_name: camera_name.to_string(),
        };
        let cfg_yaml = serde_yaml::to_string(&april_config).unwrap();
        writeln!(
            fd,
            "# The homography matrix entries (h00,...) are described in the April Tags paper"
        )?;
        writeln!(
            fd,
            "# https://dx.doi.org/10.1109/ICRA.2011.5979561 . Entry h22 is not saved because"
        )?;
        writeln!(
            fd,
            "# it always has value 1. The center pixel of the detection is (h02,h12)."
        )?;
        writeln!(fd, "# -- start of yaml config --")?;
        for line in cfg_yaml.lines() {
            writeln!(fd, "# {}", line)?;
        }
        writeln!(fd, "# -- end of yaml config --")?;

        let wtr = csv::Writer::from_writer(fd);

        Ok(Self { wtr, t0: now })
    }
    fn save(
        &mut self,
        detections: &apriltag::Zarray<apriltag::Detection>,
        frame: usize,
        ts: chrono::DateTime<chrono::Utc>,
    ) -> Result<()> {
        let time_microseconds = ts
            .signed_duration_since(self.t0)
            .num_microseconds()
            .unwrap();
        for det in detections.as_slice().iter() {
            let atd: DetectionSerializer = to_serializer(det, frame, time_microseconds);
            self.wtr.serialize(atd)?;
        }
        Ok(())
    }
}

struct FlydraConfigState {
    #[allow(dead_code)]
    region: video_streaming::Shape,
    #[cfg(feature = "flydratrax")]
    kalman_tracking_config: KalmanTrackingConfig,
}

// We perform image analysis in its own thread.
// We want to remove rustfmt::skip attribute. There is a bug similar to
// https://github.com/rust-lang/rustfmt/issues/4109 which prevents this. Bug
// 4109 does not seem exactly correct (at least presuming this was fixed in
// rustfmt 1.4.24-stable (eb894d53 2020-11-05)), but I have not found the
// correct bug.
#[rustfmt::skip]
#[allow(unused_mut,unused_variables)]
fn frame_process_thread(
    my_runtime: tokio::runtime::Handle,
    #[cfg(feature="flydratrax")]
    model_server: flydra2::ModelServer,
    #[cfg(feature="flydratrax")]
    flydratrax_calibration_source: CalSource,
    cam_name: RawCamName,
    camera_cfg: CameraCfgFview2_0_26,
    width: u32,
    height: u32,
    pixel_format: PixFmt,
    incoming_frame_rx: channellib::Receiver<Msg>,
    cam_args_tx: mpsc::Sender<CamArg>,
    #[cfg(feature="image_tracker")]
    cfg: ImPtDetectCfg,
    csv_save_pathbuf: std::path::PathBuf,
    firehose_tx: channellib::Sender<AnnotatedFrame>,
    plugin_handler_thread_tx: channellib::Sender<DynamicFrame>,
    plugin_result_rx:  channellib::Receiver<Vec<http_video_streaming_types::Point>>,
    plugin_wait_dur: std::time::Duration,
    camtrig_tx_std: channellib::Sender<ToCamtrigDevice>,
    flag: thread_control::Flag,
    is_starting: Arc<bool>,
    http_camserver_info: BuiServerInfo,
    use_cbor_packets: bool,
    process_frame_priority: Option<(i32,i32)>,
    ros_periodic_update_interval: std::time::Duration,
    #[cfg(feature = "debug-images")]
    debug_addr: std::net::SocketAddr,
    mainbrain_internal_addr: Option<MainbrainBuiLocation>,
    camdata_addr: Option<RealtimePointsDestAddr>,
    camtrig_heartbeat_update_arc: Arc<RwLock<std::time::Instant>>,
    do_process_frame_callback: bool,
    collected_corners_arc: Arc<RwLock<Vec<Vec<(f32,f32)>>>>,
    save_empty_data2d: SaveEmptyData2dType,
    valve: stream_cancel::Valve,
    #[cfg(feature = "debug-images")]
    debug_image_server_shutdown_rx: Option<tokio::sync::oneshot::Receiver<()>>,
) -> anyhow::Result<()>
{

    let ros_cam_name: RosCamName = cam_name.to_ros();

    #[cfg(feature = "posix_sched_fifo")]
    {
        if let Some((policy, priority)) = process_frame_priority {
            posix_scheduler::sched_setscheduler(0, policy, priority)?;
            info!("Frame processing thread called POSIX sched_setscheduler() \
                with policy {} priority {}", policy, priority);
        } else {
            info!("Frame processing thread using \
                default posix scheduler settings.");
        }
    }

    #[cfg(not(feature = "posix_sched_fifo"))]
    {
        if process_frame_priority.is_some() {
            panic!("Cannot set process frame priority because no support
                was compiled in.");
        } else {
            info!("Frame processing thread not configured to set posix scheduler.");
        }
    }

    #[cfg(feature="flydratrax")]
    let mut maybe_flydra2_stream = None;
    #[cfg(feature="flydratrax")]
    let mut maybe_flydra2_write_control = None;

    #[cfg_attr(not(feature = "image_tracker"), allow(dead_code))]
    struct CsvSavingState {
        fd: File,
        min_interval: chrono::Duration,
        last_save: chrono::DateTime<chrono::Utc>,
        t0: chrono::DateTime<chrono::Utc>,
    }

    // CSV saving
    #[cfg_attr(not(feature = "image_tracker"), allow(dead_code))]
    enum SavingState {
        NotSaving,
        Starting(Option<f32>),
        Saving(CsvSavingState),
    }

    #[cfg(feature="fiducial")]
    let mut apriltag_writer: Option<_> = None;
    #[cfg(not(feature="fiducial"))]
    let mut apriltag_writer: Option<()> = None;
    let mut my_mkv_writer: Option<bg_movie_writer::BgMovieWriter> = None;
    let mut fmf_writer: Option<FmfWriteInfo<_>> = None;
    #[cfg(feature="image_tracker")]
    let mut ufmf_state: Option<UfmfState> = Some(UfmfState::Stopped);
    #[cfg(feature="image_tracker")]
    #[allow(unused_assignments)]
    let mut is_doing_object_detection = false;
    let version_str = env!("CARGO_PKG_VERSION").to_string();

    #[allow(unused_mut)]
    #[allow(unused_assignments)]
    let mut frame_offset = Some(0);

    #[cfg(feature = "initially-unsychronized")]
    {
        // We start initially unsynchronized. We wait for synchronizaton.
        frame_offset = None;
    }

    let (mut transmit_current_image_tx, transmit_current_image_rx) =
        mpsc::channel::<Vec<u8>>(10);
    let http_camserver = CamHttpServerInfo::Server(http_camserver_info.clone());
    #[cfg(feature="image_tracker")]
    let mut im_tracker = FlyTracker::new(&my_runtime, &cam_name, width, height, cfg,
        Some(cam_args_tx.clone()), version_str, frame_offset, http_camserver,
        use_cbor_packets, ros_periodic_update_interval,
        #[cfg(feature = "debug-images")]
        debug_addr,
        mainbrain_internal_addr, camdata_addr, transmit_current_image_rx,
        valve.clone(),
        #[cfg(feature = "debug-images")]
        debug_image_server_shutdown_rx,
    )?;
    let mut csv_save_state = SavingState::NotSaving;
    let mut shared_store_arc: Option<Arc<RwLock<ChangeTracker<StoreType>>>> = None;
    let mut fps_calc = FpsCalc::new(100); // average 100 frames to get mean fps
    #[cfg(feature="flydratrax")]
    #[allow(unused_assignments)]
    let mut kalman_tracking_config = KalmanTrackingConfig::default(); // this is replaced below
    #[cfg(feature="flydratrax")]
    #[allow(unused_assignments)]
    let mut led_program_config = LedProgramConfig::default(); // this is replaced below
    let mut led_state = false;
    let mut current_flydra_config_state: Option<FlydraConfigState> = None;
    let mut dirty_flydra = false;
    #[cfg(feature="flydratrax")]
    #[allow(unused_assignments)]
    let mut current_led_program_config_state: Option<LedProgramConfig> = None;
    let mut dirty_led_program = false;

    let red_style = StrokeStyle::from_rgb(255, 100, 100);

    let expected_framerate_arc = Arc::new(RwLock::new(None));

    std::mem::drop(is_starting); // signal that we are we are no longer starting

    #[cfg(feature = "start-object-detection")]
    {
        is_doing_object_detection = true;
    }

    let mut post_trig_buffer = post_trigger_buffer::PostTriggerBuffer::new();

    #[cfg(feature="fiducial")]
    let mut april_td = apriltag::Detector::new();

    #[cfg(feature="fiducial")]
    let mut current_tag_family = ci2_remote_control::TagFamily::default();
    #[cfg(feature="fiducial")]
    let april_tf = make_family(&current_tag_family);
    #[cfg(feature="fiducial")]
    april_td.add_family(april_tf);

    #[cfg(feature = "checkercal")]
    let mut last_checkerboard_detection = std::time::Instant::now();

    // This limits the frequency at which the checkerboard detection routine is
    // called. This is meant to both prevent flooding the calibration routine
    // with many highly similar checkerboard images and also to allow the image
    // processing thread to keep a low queue depth on incoming frames. In the
    // current form here, however, keeping a low queue depth is dependent on the
    // checkerboard detection function returning fairly quickly. I have observed
    // the OpenCV routine taking ~90 seconds even though usually it takes 100
    // msec. Thus, this requirement is not always met. We could move this
    // checkerboard detection routine to a different thread (e.g. using a tokio
    // work pool) to avoid this problem.
    #[cfg(feature = "checkercal")]
    let mut checkerboard_loop_dur = std::time::Duration::from_millis(500);

    let current_image_timer_arc = Arc::new(RwLock::new(std::time::Instant::now()));

    let mut im_ops_socket: Option<std::net::UdpSocket> = None;

    while flag.alive() {
        #[cfg(feature="image_tracker")]
        {
            if let Some(ref ssa) = shared_store_arc {
                match ssa.try_read() {
                    Some(store) => {
                        let tracker = store.as_ref();
                        is_doing_object_detection = tracker.is_doing_object_detection; // make copy. TODO only copy on change.
                    }
                    None => {}
                }
            }
        }

        #[cfg(feature="flydratrax")]
        {
            if dirty_flydra {
                // stop flydra if things changed, will be restarted on next frame.
                is_doing_object_detection = false;
                current_flydra_config_state = None;
                dirty_flydra = false;
            }

            if dirty_led_program {
                current_led_program_config_state = None;
                dirty_led_program = false;
            }

            let kalman_tracking_enabled = if let Some(ref ssa) = shared_store_arc {
                let mut tracker = ssa.read();
                tracker.as_ref().kalman_tracking_config.enabled
            } else {
                false
            };

            // start kalman tracking if we are doing object detection but not kalman tracking yet
            // TODO if kalman_tracking_config or
            // im_pt_detect_cfg.valid_region changes, restart tracker.
            if is_doing_object_detection && maybe_flydra2_stream.is_none() {
                if let Some(ref ssa) = shared_store_arc {
                    let region = {
                        let mut tracker = ssa.write();
                        kalman_tracking_config = tracker.as_ref().kalman_tracking_config.clone();
                        led_program_config = tracker.as_ref().led_program_config.clone();
                        tracker.as_ref().im_pt_detect_cfg.valid_region.clone()
                    };
                    if kalman_tracking_enabled {
                        current_flydra_config_state = Some(FlydraConfigState {
                            region: region.clone(),
                            kalman_tracking_config: kalman_tracking_config.clone(),
                        });
                        current_led_program_config_state = Some(led_program_config.clone());
                        match region {
                            video_streaming::Shape::Polygon(_points) => {
                                unimplemented!();
                            }
                            video_streaming::Shape::Circle(circ) => {
                                let recon = match &flydratrax_calibration_source {
                                    CalSource::PseudoCal => {
                                        let cal_data = PseudoCameraCalibrationData {
                                            cam_name: cam_name.clone(),
                                            width,
                                            height,
                                            physical_diameter_meters: kalman_tracking_config.arena_diameter_meters,
                                            image_circle: circ,
                                        };
                                        cal_data.to_camera_system()?
                                    }
                                    CalSource::XmlFile(cal_fname) => {
                                        let rdr = std::fs::File::open(&cal_fname)?;
                                        flydra_mvg::FlydraMultiCameraSystem::from_flydra_xml(rdr)?
                                    }
                                    CalSource::PymvgJsonFile(cal_fname) => {
                                        let rdr = std::fs::File::open(&cal_fname)?;
                                        let sys = mvg::MultiCameraSystem::from_pymvg_file_json(rdr)?;
                                        flydra_mvg::FlydraMultiCameraSystem::from_system(sys, None)
                                    }
                                };

                                let (save_data_tx, save_data_rx) = channellib::unbounded();
                                maybe_flydra2_write_control = Some(CoordProcessorControl::new(save_data_tx.clone()));
                                let (flydra2_tx, flydra2_rx) = futures::channel::mpsc::channel(100);

                                let (model_sender, model_receiver) = channellib::unbounded();

                                let kalman_tracking_config2 = kalman_tracking_config.clone();
                                let camtrig_tx_std2 = camtrig_tx_std.clone();
                                let ssa2 = ssa.clone();
                                let cam_args_tx2 = cam_args_tx.clone();

                                assert_eq!(recon.len(), 1); // TODO: check if camera name in system and allow that?
                                let cam_cal = recon.cameras().next().unwrap().to_cam();

                                // TODO: add flag and control to kill thread on shutdown
                                // TODO: convert this to a future on our runtime?
                                std::thread::Builder::new().name("flydratrax_handle_msg".to_string()).spawn(move || { // flydratrax ignore for now
                                    let thread_closer = CloseAppOnThreadExit::new(cam_args_tx2, file!(), line!());
                                    // let cam_cal = thread_closer.check(cal_data.to_cam().map_err(|e| anyhow::Error::new(Box::new(e)))); // camera calibration
                                    let kalman_tracking_config = kalman_tracking_config2.clone();
                                    thread_closer.maybe_err(flydratrax_handle_msg::flydratrax_handle_msg(cam_cal,
                                            model_receiver,
                                            &mut led_state, ssa2, camtrig_tx_std2,
                                            ).map_err(|e| anyhow::Error::new(Box::new(e))));
                                })?;

                                let expected_framerate_arc2 = expected_framerate_arc.clone();
                                let cam_name2 = cam_name.clone();
                                let http_camserver = CamHttpServerInfo::Server(
                                    http_camserver_info.clone());
                                let recon2 = recon.clone();
                                let model_server2 = model_server.clone();
                                let valve2 = valve.clone();

                                let cam_manager = flydra2::ConnectedCamerasManager::new_single_cam(&cam_name2,
                                    &http_camserver, &Some(recon2));
                                let tracking_params = flydra2::SwitchingTrackingParams::default();
                                let ignore_latency = false;
                                let mut coord_processor = CoordProcessor::new(
                                    cam_manager, Some(recon),
                                    tracking_params,
                                    save_data_tx,
                                    save_data_rx, save_empty_data2d, ignore_latency)
                                    .expect("create CoordProcessor");

                                let flydratrax_server = crate::flydratrax_handle_msg::FlydraTraxServer::new(model_sender);

                                coord_processor.add_listener(Box::new(flydratrax_server)); // the local LED control thing
                                coord_processor.add_listener(Box::new(model_server2)); // the HTTP thing

                                let expected_framerate = *expected_framerate_arc2.read();
                                let flydra2_rx_valved = valve2.wrap(flydra2_rx);
                                let consume_future = coord_processor.consume_stream(flydra2_rx_valved,
                                    expected_framerate);

                                use futures::future::FutureExt;
                                let consume_future_noerr = consume_future.map( |opt_jh| {
                                    if let Some(jh) = opt_jh {
                                        debug!("waiting on flydratrax coord processor {}:{}", file!(), line!());
                                        jh.join().expect("join consume_future_noerr");
                                        debug!("done waiting on flydratrax coord processor {}:{}", file!(), line!());
                                    }
                                    debug!("consume future noerr finished {}:{}", file!(), line!());
                                });

                                my_runtime.spawn(consume_future_noerr); // flydratrax ignore for now
                                maybe_flydra2_stream = Some(flydra2_tx);
                            },
                            video_streaming::Shape::Everything => {
                                error!("cannot start tracking without circular region to use as camera calibration");
                            },
                        }
                    }
                }
            }

            if !is_doing_object_detection | !kalman_tracking_enabled {
                // drop all flydra2 stuff if we are not tracking
                maybe_flydra2_stream = None;
                if let Some(ref mut write_controller) = maybe_flydra2_write_control {
                    write_controller.stop_saving_data();
                }
                maybe_flydra2_write_control = None;
            }

        }

        let msg = match incoming_frame_rx.recv() {
            Ok(msg) => msg,
            Err(channellib::RecvError{..}) => {
                info!("incoming frame channel closed for '{}'", cam_name.as_str());
                break;
            },
        };
        let store_cache = if let Some(ref ssa) = shared_store_arc {
            let mut tracker = ssa.read();
            Some(tracker.as_ref().clone())
        } else {
            None
        };

        if let Some(ref store_cache_ref) = store_cache {

            #[cfg(feature="flydratrax")]
            {
                if let Some(ref cfcs) = current_flydra_config_state {
                    if store_cache_ref.kalman_tracking_config != cfcs.kalman_tracking_config {
                        dirty_flydra = true;
                    }
                    if store_cache_ref.im_pt_detect_cfg.valid_region != cfcs.region {
                        dirty_flydra = true;
                    }
                }
                if let Some(ref clpcs) = current_led_program_config_state {
                    if &store_cache_ref.led_program_config != clpcs {
                        dirty_led_program = true;
                    }
                }
            }

        }

        match msg {
            Msg::Store(stor) => {
                // We get the shared store once at startup.
                #[cfg(feature = "start-object-detection")]
                {
                    let mut tracker = stor.write();
                    tracker.modify(|tracker| {
                        tracker.is_doing_object_detection = true;
                    });
                }
                {
                    let tracker = stor.read();
                    let shared = tracker.as_ref();
                    post_trig_buffer.set_size(shared.post_trigger_buffer_size);
                }
                shared_store_arc = Some(stor);
            }
            Msg::StartFMF((dest,recording_framerate)) => {
                let path = Path::new(&dest);
                let f = std::fs::File::create(&path)?;
                fmf_writer = Some(FmfWriteInfo::new(FMFWriter::new(f)?, recording_framerate));
            }
            Msg::StartMkv((format_str_mkv,mkv_recording_config)) => {
                my_mkv_writer = Some(bg_movie_writer::BgMovieWriter::new_webm_writer(format_str_mkv, mkv_recording_config, 100));
            }
            #[cfg(feature="image_tracker")]
            Msg::StartUFMF(dest) => {
                ufmf_state = Some(UfmfState::Starting(dest));
            }
            Msg::PostTriggerStartMkv((format_str_mkv,mkv_recording_config)) => {
                let frames = post_trig_buffer.get_and_clear();
                let mut raw = bg_movie_writer::BgMovieWriter::new_webm_writer(format_str_mkv, mkv_recording_config, frames.len()+100);
                for mut frame in frames.into_iter() {
                    // Force frame width to be power of 2.
                    let val = 2;
                    let clipped_width = (frame.width() / val as u32) * val as u32;
                    match_all_dynamic_fmts!(&mut frame, x, {x.width = clipped_width});
                    // frame.width = clipped_width;
                    let ts = frame.extra().host_timestamp();
                    raw.write(frame, ts)?;
                }
                my_mkv_writer = Some(raw);
            }
            Msg::StartAprilTagRec(format_str_apriltags_csv) => {
                #[cfg(feature="fiducial")]
                {
                    if let Some(x) = store_cache.as_ref() {
                        if let Some(apriltag_state) = &x.apriltag_state {
                            apriltag_writer = Some(AprilTagWriter::new(format_str_apriltags_csv, &x.camera_name)?);
                        }
                    }
                }
            }
            Msg::StopAprilTagRec => {
                #[allow(unused_assignments)]
                {
                    apriltag_writer = None;
                }
            }
            Msg::SetPostTriggerBufferSize(size) => {
                post_trig_buffer.set_size(size);
                if let Some(ref mut store) = shared_store_arc {
                    let mut tracker = store.write();
                    tracker.modify(|tracker| {
                        tracker.post_trigger_buffer_size = size;
                    });
                }
            }
            Msg::Mframe(frame) => {
                let extra = frame.extra();
                if let Some(new_fps) = fps_calc
                    .update(extra.host_framenumber(), extra.host_timestamp()) {
                    if let Some(ref mut store) = shared_store_arc {
                        let mut tracker = store.write();
                        tracker.modify(|tracker| {
                            tracker.measured_fps = new_fps as f32;
                        });
                    }

                    {
                        let mut expected_framerate = expected_framerate_arc.write();
                        *expected_framerate = Some(new_fps as f32);
                    }
                }

                post_trig_buffer.push(&frame); // If buffer size larger than 0, copies data.

                #[cfg(feature="checkercal")]
                let checkercal_tmp = store_cache.as_ref().and_then(|x|
                    if x.checkerboard_data.enabled {
                        Some((x.checkerboard_data.clone(),x.checkerboard_save_debug.clone()))
                    } else {
                        None
                    });

                #[cfg(not(feature="checkercal"))]
                let checkercal_tmp: Option<()> = None;

                let (mut found_points, valid_display) = if let Some(inner) = checkercal_tmp {
                    let mut results = Vec::new();
                    #[cfg(feature="checkercal")]
                    {
                        let (checkerboard_data,checkerboard_save_debug) = inner;

                        // do not do this too often
                        if last_checkerboard_detection.elapsed() > checkerboard_loop_dur {

                            let debug_image_stamp: chrono::DateTime<chrono::Local> = chrono::Local::now();
                            if let Some(debug_dir) = &checkerboard_save_debug {
                                let format_str = format!("input_{}_{}_%Y%m%d_%H%M%S.png",
                                    checkerboard_data.width, checkerboard_data.height);
                                let stamped = debug_image_stamp.format(&format_str).to_string();
                                let png_buf = match_all_dynamic_fmts!(&frame, x, {
                                    convert_image::frame_to_image(x, convert_image::ImageOptions::Png)?
                                });

                                let debug_path = std::path::PathBuf::from(debug_dir);
                                let image_path = debug_path.join(stamped);

                                let mut f = File::create(
                                    &image_path)
                                    .expect("create file");
                                f.write_all(&png_buf).unwrap();
                            }

                            let start_time = std::time::Instant::now();

                            info!("Attempting to find {}x{} chessboard.",
                            checkerboard_data.width, checkerboard_data.height);

                            let corners = basic_frame::match_all_dynamic_fmts!(&frame, x, {
                                let rgb: Box<dyn formats::ImageStride<formats::pixel_format::RGB8>> =
                                Box::new(convert_image::convert::<_,formats::pixel_format::RGB8>(x)?);
                                let corners = opencv_calibrate::find_chessboard_corners(
                                    rgb.image_data(),
                                    rgb.width(), rgb.height(),
                                    checkerboard_data.width as usize, checkerboard_data.height as usize,
                                    )?;
                                corners
                            });


                            let work_duration = start_time.elapsed();
                            if work_duration > checkerboard_loop_dur {
                                checkerboard_loop_dur = work_duration + std::time::Duration::from_millis(5);
                            }
                            last_checkerboard_detection = std::time::Instant::now();

                            debug!("corners: {:?}", corners);

                            if let Some(debug_dir) = &checkerboard_save_debug {
                                let format_str = "input_%Y%m%d_%H%M%S.yaml";
                                let stamped = debug_image_stamp.format(&format_str).to_string();

                                let debug_path = std::path::PathBuf::from(debug_dir);
                                let yaml_path = debug_path.join(stamped);

                                let mut f = File::create(
                                    &yaml_path)
                                    .expect("create file");

                                #[derive(Serialize)]
                                struct CornerData<'a> {
                                    corners: &'a Option<Vec<(f32, f32)>>,
                                    work_duration: std::time::Duration,
                                }
                                let debug_data = CornerData {
                                    corners: &corners,
                                    work_duration,
                                };

                                serde_yaml::to_writer(f, &debug_data)
                                    .expect("serde_yaml::to_writer");
                            }

                            if let Some(corners) = corners {
                                info!("Found {} chessboard corners in {} msec.", corners.len(), work_duration.as_millis());
                                results = corners.iter().map(|(x,y)| {
                                    video_streaming::Point {
                                        x: *x,
                                        y: *y,
                                        theta: None,
                                        area: None,
                                    }
                                }).collect();

                                let num_checkerboards_collected = {
                                    let mut collected_corners = collected_corners_arc.write();
                                    collected_corners.push(corners);
                                    collected_corners.len().try_into().unwrap()
                                };

                                if let Some(ref ssa) = shared_store_arc {
                                    // scope for write lock on ssa
                                    let mut tracker = ssa.write();
                                    tracker.modify(|shared| {
                                        shared.checkerboard_data.num_checkerboards_collected = num_checkerboards_collected;
                                    });
                                }
                            } else {
                                info!("Found no chessboard corners in {} msec.", work_duration.as_millis());
                            }
                        }
                    }
                    (results, None)
                } else {

                    let mut all_points = Vec::new();
                    let mut blkajdsfads = None;

                    {
                        if let Some(ref store_cache_ref) = store_cache {
                            if store_cache_ref.im_ops_state.do_detection {

                                let thresholded = if let DynamicFrame::Mono8(mono8) = &frame {
                                    imops::threshold(
                                        mono8.clone(),
                                        imops::CmpOp::LessThan,
                                        store_cache_ref.im_ops_state.threshold,
                                        0,
                                        255)
                                } else {
                                    panic!("imops only implemented for Mono8 pixel format");
                                };
                                let mu00 = imops::spatial_moment_00(&thresholded);
                                let mu01 = imops::spatial_moment_01(&thresholded);
                                let mu10 = imops::spatial_moment_10(&thresholded);
                                let mc = if mu00 != 0.0 {

                                    let x = mu01 / mu00;
                                    let y = mu10 / mu00;

                                    // If mu00 is 0.0, these will be NaN. CBOR explicitly can represent NaNs.

                                    let mc = ToDevice::Centroid(MomentCentroid {
                                        x,
                                        y,
                                        center_x: store_cache_ref.im_ops_state.center_x,
                                        center_y: store_cache_ref.im_ops_state.center_y,
                                    });
                                    all_points.push(video_streaming::Point {x, y, area: None, theta: None});

                                    Some(mc)
                                } else {
                                    None
                                };


                                let need_new_socket = if let Some(socket) = &im_ops_socket {
                                    if socket.local_addr().unwrap().ip() == store_cache_ref.im_ops_state.source {
                                        // Source IP remained constant.
                                        false
                                    } else {
                                        true
                                    }
                                } else {
                                    true
                                };

                                if need_new_socket {
                                    let mut iter = std::net::ToSocketAddrs::to_socket_addrs(&(store_cache_ref.im_ops_state.source, 0u16)).unwrap();
                                    let sockaddr = iter.next().unwrap();

                                    im_ops_socket = std::net::UdpSocket::bind(sockaddr).map_err(|e| {error!("failed opening socket: {}", e); ()}).ok();
                                }

                                if let Some(socket) = &mut im_ops_socket {
                                    if let Some(mc) = mc {
                                        let buf = serde_cbor::to_vec(&mc).unwrap();
                                        match socket.send_to(&buf, &store_cache_ref.im_ops_state.destination) {
                                            Ok(_n_bytes) => {},
                                            Err(e) => {
                                                log::error!("Unable to send image moment data. {}", e);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    #[cfg(feature="fiducial")]
                    {

                        if let Some(ref store_cache_ref) = store_cache {

                            if let Some(ref ts) = store_cache_ref.apriltag_state {

                                if ts.do_detection {
                                    use apriltag::ImageU8;

                                    if current_tag_family != ts.april_family {
                                        april_td.clear_families();
                                        current_tag_family = ts.april_family.clone();
                                        let april_tf = make_family(&current_tag_family);
                                        april_td.add_family(april_tf);
                                    }

                                    if let Some(mut im) = frame2april(&frame) {
                                        let detections = april_td.detect(im.inner_mut());

                                        if let Some(ref mut wtr) = apriltag_writer {
                                            wtr.save(&detections, frame.extra().host_framenumber(), frame.extra().host_timestamp())?;
                                        }

                                        let tag_points = detections.as_slice().iter().map(det2display);
                                        all_points.extend(tag_points);
                                    }
                                }
                            }
                        }
                    }

                    #[cfg(feature="image_tracker")]
                    {
                    if is_doing_object_detection {
                        let inner_ufmf_state = ufmf_state.take().unwrap();
                        let (tracker_annotation, new_ufmf_state) = im_tracker.process_new_frame(&frame, inner_ufmf_state)?;
                        ufmf_state.get_or_insert(new_ufmf_state);

                        #[cfg(feature="flydratrax")]
                        {
                            if let Some(ref mut flydra2_stream) = maybe_flydra2_stream {
                                let points = tracker_annotation.points.iter()
                                    .filter(|pt| pt.area >= kalman_tracking_config.min_central_moment as f64)
                                    .enumerate().map(|(i,pt)| {
                                        assert!(i <= u8::max_value() as usize);
                                        let idx = i as u8;
                                        flydra2::NumberedRawUdpPoint {
                                            idx,
                                            pt: pt.clone(),
                                        }
                                    }).collect();

                                let cam_received_timestamp = datetime_conversion::datetime_to_f64(
                                    &frame.extra().host_timestamp());

                                // TODO FIXME XXX It is a lie that this timesource is Triggerbox.
                                let trigger_timestamp = Some(FlydraFloatTimestampLocal::<Triggerbox>::from_f64(
                                    cam_received_timestamp));

                                // This is not a lie.
                                let cam_received_timestamp = FlydraFloatTimestampLocal::<HostClock>::from_f64(
                                    cam_received_timestamp);

                                let cam_num = 0.into(); // Only one camera, so this must be correct.
                                let frame_data = flydra2::FrameData::new(
                                    ros_cam_name.clone(),
                                    cam_num,
                                    SyncFno(frame.extra().host_framenumber() as u64),
                                    trigger_timestamp,
                                    cam_received_timestamp,
                                );
                                let fdp = flydra2::FrameDataAndPoints{
                                    frame_data,
                                    points,
                                };
                                let si = StreamItem::Packet(fdp);

                                // block until sent
                                match futures::executor::block_on( futures::sink::SinkExt::send( flydra2_stream, si)) {
                                    Ok(()) => {},
                                    Err(e) => return Err(e.into()),
                                }

                            }
                        }

                        let points = tracker_annotation.points;

                        let mut new_state = None;
                        match csv_save_state {
                            SavingState::NotSaving => {}
                            SavingState::Starting(rate_limit) => {
                                // create dir if needed
                                std::fs::create_dir_all(&csv_save_pathbuf)?;

                                // start saving tracking
                                let base_template = "flytrax%Y%m%d_%H%M%S";
                                let now = frame.extra().host_timestamp();
                                let local = now.with_timezone(&chrono::Local);
                                let base = local.format(base_template).to_string();

                                // save jpeg image
                                {
                                    let mut image_path = csv_save_pathbuf.clone();
                                    image_path.push(base.clone());
                                    image_path.set_extension("jpg");

                                    let bytes = match_all_dynamic_fmts!(&frame,x, {convert_image::frame_to_image(x,
                                        convert_image::ImageOptions::Jpeg(99))?});
                                    File::create(image_path)?
                                        .write_all(&bytes)?;
                                }

                                let mut csv_path = csv_save_pathbuf.clone();
                                csv_path.push(base);
                                csv_path.set_extension("csv");
                                info!("saving data to {}.", csv_path.display());

                                if let Some(ref ssa) = shared_store_arc {
                                    // scope for write lock on ssa
                                    let new_val = RecordingPath::new(csv_path.display().to_string());
                                    let mut tracker = ssa.write();
                                    tracker.modify(|shared| {
                                        shared.is_saving_im_pt_detect_csv = Some(new_val);
                                    });
                                }

                                let mut fd = File::create(csv_path)?;

                                // save configuration as commented yaml
                                {
                                    let save_cfg = SaveCfgFview2_0_25 {
                                        name: env!("APP_NAME").to_string(),
                                        version: env!("CARGO_PKG_VERSION").to_string(),
                                        git_hash: env!("GIT_HASH").to_string(),
                                    };

                                    let cfg_clone = im_tracker.config();

                                    let full_cfg = FullCfgFview2_0_26 {
                                        app: save_cfg,
                                        camera: camera_cfg.clone(),
                                        created_at: local,
                                        csv_rate_limit: rate_limit,
                                        object_detection_cfg: im_tracker.config().clone(),
                                    };
                                    let cfg_yaml = serde_yaml::to_string(&full_cfg).unwrap();
                                    writeln!(fd, "# -- start of yaml config --")?;
                                    for line in cfg_yaml.lines() {
                                        writeln!(fd, "# {}", line)?;
                                    }
                                    writeln!(fd, "# -- end of yaml config --")?;
                                }

                                writeln!(fd, "{},{},{},{},{},{},{},{},{}",
                                    "time_microseconds", "frame", "x_px",
                                    "y_px", "orientation_radians_mod_pi", "central_moment", "led_1", "led_2",
                                    "led_3")?;
                                fd.flush()?;

                                let min_interval_sec = if let Some(fps) = rate_limit {
                                    1.0 / fps
                                } else {
                                    0.0
                                };
                                let min_interval = chrono::Duration::nanoseconds((min_interval_sec*1e9) as i64);

                                let inner = CsvSavingState {
                                    fd,
                                    min_interval,
                                    last_save: now.checked_sub_signed(chrono::Duration::days(1)).unwrap(),
                                    t0: now,
                                };

                                new_state = Some(SavingState::Saving(inner));

                            }
                            SavingState::Saving(ref mut inner) => {
                                let interval = frame.extra().host_timestamp().signed_duration_since(inner.last_save);
                                // save found points
                                if interval >= inner.min_interval && points.len() >= 1 {
                                    let time_microseconds = frame.extra().host_timestamp()
                                        .signed_duration_since(inner.t0)
                                        .num_microseconds().unwrap();

                                    let mut led1 = "".to_string();
                                    let mut led2 = "".to_string();
                                    let mut led3 = "".to_string();
                                    #[cfg(feature="with_camtrig")]
                                    {
                                        if let Some(ref store) = store_cache {
                                            if let Some(ref device_state) = store.camtrig_device_state {
                                                led1 = format!("{}",get_intensity(&device_state,1));
                                                led2 = format!("{}",get_intensity(&device_state,2));
                                                led3 = format!("{}",get_intensity(&device_state,3));
                                            }
                                        }
                                    }
                                    for pt in points.iter() {
                                        let orientation_mod_pi = match pt.maybe_slope_eccentricty {
                                            Some((slope,_ecc)) => {
                                                let orientation_mod_pi = f32::atan(slope as f32);
                                                format!("{:.3}", orientation_mod_pi)
                                            },
                                            None => "".to_string(),
                                        };
                                        writeln!(inner.fd,
                                            "{},{},{:.1},{:.1},{},{},{},{},{}",
                                            time_microseconds, frame.extra().host_framenumber(),
                                            pt.x0_abs, pt.y0_abs, orientation_mod_pi,
                                            pt.area, led1, led2, led3)?;
                                        inner.fd.flush()?;
                                    }
                                    inner.last_save = frame.extra().host_timestamp();
                                }
                            }
                        }
                        if let Some(ns) = new_state {
                            csv_save_state = ns;
                        }

                        let display_points: Vec<_> = points
                            .iter()
                            .map(|pt| {
                                video_streaming::Point {
                                    x: pt.x0_abs as f32,
                                    y: pt.y0_abs as f32,
                                    theta: pt.maybe_slope_eccentricty.and_then(|(slope,_ecc)| Some(f32::atan(slope as f32))),
                                    area: Some(pt.area as f32),
                                }
                            })
                            .collect();

                        all_points.extend(display_points);
                        blkajdsfads = Some(im_tracker.valid_region())
                    }
                    }
                    (all_points, blkajdsfads)
                };

                if let Some(ref mut inner) = my_mkv_writer {
                    let data = frame.clone(); // copy entire frame data
                    inner.write(data, frame.extra().host_timestamp())?;
                }

                if let Some(ref mut inner) = fmf_writer {
                    let do_save = match inner.last_saved_stamp {
                        None => true,
                        Some(stamp) => stamp.elapsed() >= inner.recording_framerate.interval(),
                    };
                    if do_save {
                        match_all_dynamic_fmts!(&frame, x, {
                            inner.writer.write(x, frame.extra().host_timestamp())?
                        });
                        inner.last_saved_stamp = Some(std::time::Instant::now());
                    }
                }

                #[cfg(feature="plugin-process-frame")]
                {
                    // Do FFI image processing with lowest latency possible
                    if do_process_frame_callback {
                        if plugin_handler_thread_tx.is_full() {
                            error!("cannot transmit frame to plugin: channel full");
                        }  else {
                            plugin_handler_thread_tx.send(frame.clone()).cb_ok();
                            match plugin_result_rx.recv_timeout(plugin_wait_dur) {
                                Ok(results) => {
                                    found_points.extend(results);
                                }
                                Err(e) => {
                                    if e.is_timeout() {
                                        error!("Not displaying annotation because the plugin took too long.");
                                    } else {
                                        error!("The plugin disconnected.");
                                        return Err(StrandCamError::PluginDisconnected.into());
                                    }
                                }
                            }
                        }
                    }
                }

                let found_points = found_points
                    .iter()
                    .map(|pt: &http_video_streaming_types::Point| {
                        video_streaming::Point {
                            x: pt.x,
                            y: pt.y,
                            theta: pt.theta,
                            area: pt.area,
                        }
                    })
                    .collect();

                #[cfg(feature="send-bg-images-to-mainbrain")]
                {
                    // send current image every 2 seconds
                    let mut timer = current_image_timer_arc.write();
                    let elapsed = timer.elapsed();
                    if elapsed > std::time::Duration::from_millis(2000) {

                        *timer = std::time::Instant::now();
                        // encode frame to png buf

                        let buf = match_all_dynamic_fmts!(&frame, x, {
                            convert_image::frame_to_image(x, convert_image::ImageOptions::Png)?
                        });

                        // send to UpdateCurrentImage
                        match transmit_current_image_tx.try_send(buf) {
                            Ok(()) => {}, // frame put in channel ok
                            Err(e) => {
                                if e.is_full() {
                                    // channel was full
                                    error!("not updating image on braid due to backpressure");
                                }
                                if e.is_disconnected() {
                                    debug!("update image on braid listener disconnected");
                                    return Err(StrandCamError::BraidUpdateImageListenerDisconnected.into());
                                }
                            }
                        }
                    }
                }

                #[cfg(feature="with_camtrig")]
                // check camtrig device heartbeat
                {
                    let reader = camtrig_heartbeat_update_arc.read();
                    let elapsed = reader.elapsed();
                    if elapsed > std::time::Duration::from_millis(2*CAMTRIG_HEARTBEAT_INTERVAL_MSEC) {

                        error!("No camtrig heatbeat for {:?}.", elapsed);

                        // No heartbeat within the specified interval.
                        if let Some(ref ssa) = shared_store_arc {
                            let mut tracker = ssa.write();
                            tracker.modify(|store| store.camtrig_device_lost = true);
                        }
                    }
                }

                #[cfg(feature="flydratrax")]
                let annotations = if let Some(ref clpcs) = current_led_program_config_state {
                    vec![ DrawableShape::from_shape( &clpcs.led_on_shape_pixels, &red_style, 1.0 ) ]
                } else {
                    vec![]
                };

                #[cfg(not(feature="flydratrax"))]
                let annotations = vec![];

                let name = None;
                if firehose_tx.is_full() {
                    debug!("cannot transmit frame for viewing: channel full");
                }  else {
                    firehose_tx.send(AnnotatedFrame {
                        frame,
                        found_points,
                        valid_display,
                        annotations,
                        name,
                    }).cb_ok();
                }
            }
            #[cfg(feature="image_tracker")]
            Msg::SetIsSavingObjDetectionCsv(new_value) => {
                info!("setting object detection CSV save state to: {:?}", new_value);
                if let CsvSaveConfig::Saving(fps_limit) = new_value {
                    if !store_cache.map(|s| s.is_doing_object_detection).unwrap_or(false) {
                        error!("Not doing object detection, ignoring command to save data to CSV.");
                    } else {
                        csv_save_state = SavingState::Starting(fps_limit);

                        #[cfg(feature="flydratrax")]
                        {
                            if let Some(ref mut write_controller) = maybe_flydra2_write_control {
                                let local: chrono::DateTime<chrono::Local> = chrono::Local::now();
                                let dirname = local.format("%Y%m%d_%H%M%S.braid").to_string();
                                let mut my_dir = csv_save_pathbuf.clone();
                                my_dir.push(dirname);

                                warn!("unimplemented setting of FPS and camera images");
                                let expected_fps = None;
                                let images = flydra2::ImageDictType::new();

                                let cfg = flydra2::StartSavingCsvConfig {
                                    out_dir: my_dir.clone(),
                                    local: Some(local),
                                    git_rev: env!("GIT_HASH").to_string(),
                                    fps: expected_fps,
                                    images,
                                    print_stats: false,
                                    save_performance_histograms: true,
                                };
                                write_controller.start_saving_data(cfg);
                            }
                        }
                    }
                } else {
                    match csv_save_state {
                        SavingState::NotSaving => {}
                        _ => {info!("stopping data saving.");}
                    }
                    // this potentially drops file, thus closing it.
                    csv_save_state = SavingState::NotSaving;
                    #[cfg(feature="flydratrax")]
                    {
                        if let Some(ref mut write_controller) = maybe_flydra2_write_control {
                            write_controller.stop_saving_data();
                        }
                    }

                    // update UI
                    if let Some(ref ssa) = shared_store_arc {
                        // scope for write lock on ssa
                        let mut tracker = ssa.write();
                        tracker.modify(|shared| {
                            shared.is_saving_im_pt_detect_csv = None;
                        });
                    }
                }
            }
            #[cfg(feature="image_tracker")]
            Msg::SetExpConfig(cfg) => {
                im_tracker.set_config(cfg).expect("set_config()");
            }
            #[cfg(feature="image_tracker")]
            Msg::TakeCurrentImageAsBackground => {
                im_tracker.do_take_current_image_as_background()?;
            }
            #[cfg(feature="image_tracker")]
            Msg::ClearBackground(value) => {
                im_tracker.do_clear_background(value)?;
            }
            Msg::SetFrameOffset(fo) => {
                #[cfg(feature="image_tracker")]
                im_tracker.set_frame_offset(fo);
            }
            Msg::SetClockModel(cm) => {
                #[cfg(feature="image_tracker")]
                im_tracker.set_clock_model(cm);
            }
            Msg::StopMkv => {
                if let Some(mut inner) = my_mkv_writer.take() {
                    inner.finish()?;
                }
            }
            Msg::StopFMF => {
                fmf_writer = None;
            }
            #[cfg(feature="image_tracker")]
            Msg::StopUFMF => {
                ufmf_state = Some(UfmfState::Stopped);
            }
            #[cfg(feature="image_tracker")]
            Msg::SetTracking(value) => {
                is_doing_object_detection = value;
            }
            Msg::QuitFrameProcessThread => {
                break;
            }
        };
    }
    info!("frame process thread done for camera '{}'",cam_name.as_str());
    Ok(())
}

#[cfg(feature = "with_camtrig")]
fn get_intensity(device_state: &camtrig_comms::DeviceState, chan_num: u8) -> u16 {
    let ch: &camtrig_comms::ChannelState = match chan_num {
        1 => &device_state.ch1,
        2 => &device_state.ch2,
        3 => &device_state.ch3,
        c => panic!("unknown channel {}", c),
    };
    match ch.on_state {
        camtrig_comms::OnState::Off => 0,
        camtrig_comms::OnState::ConstantOn => ch.intensity,
        camtrig_comms::OnState::PulseTrain(_) => ch.intensity,
    }
}

pub struct NoisyDrop<T> {
    inner: T,
    name: String,
    file: String,
    line: u32,
}

impl<T> NoisyDrop<T> {
    fn new(inner: T, name: String, file: &str, line: u32) -> Self {
        debug!("Creating {} at {}:{}", name, file, line);
        Self {
            inner,
            name,
            file: file.into(),
            line,
        }
    }
}

impl<T> std::ops::Deref for NoisyDrop<T> {
    type Target = T;
    #[inline(always)]
    fn deref(&self) -> &T {
        &self.inner
    }
}

impl<T> Drop for NoisyDrop<T> {
    fn drop(&mut self) {
        debug!(
            "Dropping {} originally from {}:{}",
            self.name, self.file, self.line
        );
        #[cfg(feature = "backtrace")]
        error!("{}", std::backtrace::Backtrace::capture());
    }
}

macro_rules! noisy_drop {
    ($name:ident) => {
        NoisyDrop::new($name, stringify!($name).to_string(), file!(), line!())
    };
}

pub struct MyApp {
    inner: BuiAppInner<StoreType, CallbackType>,
    txers: Arc<RwLock<HashMap<ConnectionKey, (SessionKey, EventChunkSender, String)>>>,
}

impl MyApp {
    #![cfg_attr(not(feature = "image_tracker"), allow(unused_variables))]
    async fn new(
        rt_handle: tokio::runtime::Handle,
        shared_store_arc: Arc<RwLock<ChangeTracker<StoreType>>>,
        secret: Option<Vec<u8>>,
        http_server_addr: &str,
        config: Config,
        cam_args_tx: mpsc::Sender<CamArg>,
        camtrig_tx_std: channellib::Sender<ToCamtrigDevice>,
        tx_frame: channellib::Sender<Msg>,
        valve: stream_cancel::Valve,
        shutdown_rx: tokio::sync::oneshot::Receiver<()>,
    ) -> std::result::Result<(channellib::Receiver<FirehoseCallback>, Self), StrandCamError> {
        let chan_size = 10;

        let addr: std::net::SocketAddr = http_server_addr.parse().unwrap();
        let auth = if let Some(ref secret) = secret {
            bui_backend::highlevel::generate_random_auth(addr, secret.clone())?
        } else {
            if addr.ip().is_loopback() {
                AccessControl::Insecure(addr)
            } else {
                return Err(StrandCamError::JwtError);
            }
        };

        let (new_conn_rx, mut inner) = create_bui_app_inner(
            Some(shutdown_rx),
            &auth,
            shared_store_arc,
            config,
            chan_size,
            &strand_cam_storetype::STRAND_CAM_EVENTS_URL_PATH,
            Some(strand_cam_storetype::STRAND_CAM_EVENT_NAME.to_string()),
        )
        .await?;

        // A channel for the data send from the client browser. No need to convert to
        // bounded to prevent exploding when camera too fast.
        let (firehose_callback_tx, firehose_callback_rx) = channellib::unbounded();
        let firehose_callback_tx = noisy_drop!(firehose_callback_tx);

        debug!("created firehose_callback_tx");

        // Create a Stream to handle callbacks from clients.
        inner.set_callback_listener(Box::new(
            move |msg: CallbackDataAndSession<CallbackType>| {
                match msg.payload {
                    CallbackType::ToCamera(cam_arg) => {
                        debug!("in cb: {:?}", cam_arg);
                        cam_args_tx
                            .clone()
                            .start_send(cam_arg)
                            .expect("to_camera start_send");
                    }
                    CallbackType::FirehoseNotify(inner) => {
                        let arrival_time = chrono::Utc::now();
                        let fc = FirehoseCallback {
                            arrival_time,
                            inner,
                        };
                        firehose_callback_tx.send(fc).cb_ok();
                    }
                    CallbackType::TakeCurrentImageAsBackground => {
                        #[cfg(feature = "image_tracker")]
                        tx_frame.send(Msg::TakeCurrentImageAsBackground).cb_ok();
                    }
                    CallbackType::ClearBackground(value) => {
                        #[cfg(feature = "image_tracker")]
                        tx_frame.send(Msg::ClearBackground(value)).cb_ok();
                    }
                    CallbackType::ToCamtrig(camtrig_arg) => {
                        info!("in camtrig callback: {:?}", camtrig_arg);
                        camtrig_tx_std.send(camtrig_arg).cb_ok();
                    }
                }
                futures::future::ok(())
            },
        ));

        let txers = Arc::new(RwLock::new(HashMap::new()));
        let txers2 = txers.clone();
        let mut new_conn_rx_valved = valve.wrap(new_conn_rx);
        let new_conn_future = async move {
            while let Some(msg) = new_conn_rx_valved.next().await {
                let mut txers = txers2.write();
                match msg.typ {
                    ConnectionEventType::Connect(chunk_sender) => {
                        txers.insert(
                            msg.connection_key,
                            (msg.session_key, chunk_sender, msg.path),
                        );
                    }
                    ConnectionEventType::Disconnect => {
                        txers.remove(&msg.connection_key);
                    }
                }
            }
            debug!("new_conn_future closing {}:{}", file!(), line!());
        };
        let _task_join_handle = rt_handle.spawn(new_conn_future);

        let my_app = MyApp { inner, txers };

        Ok((firehose_callback_rx, my_app))
    }

    /// Spawn the camtrig thread (if compiled to do so).
    ///
    /// In the case of #[cfg(feature="with-camtrig")], this will spawn
    /// the serial thread that communicates with the camtrig device.
    /// Otherwise, does very little and `sjh` is essentially empty.
    fn maybe_spawn_camtrig_thread(
        &self,
        camtrig_tx_std: channellib::Sender<ToCamtrigDevice>,
        camtrig_rx: channellib::Receiver<ToCamtrigDevice>,
        camtrig_heartbeat_update_arc: Arc<RwLock<std::time::Instant>>,
        cam_args_tx: mpsc::Sender<CamArg>,
    ) -> Result<SerialJoinHandles> {
        #[cfg(feature = "with_camtrig")]
        let sjh = {
            run_camtrig(
                self.inner.shared_arc().clone(), // shared_store_arc
                camtrig_tx_std,
                camtrig_rx,
                camtrig_heartbeat_update_arc,
                cam_args_tx,
            )?
        };

        #[cfg(not(feature = "with_camtrig"))]
        let sjh = SerialJoinHandles {};
        Ok(sjh)
    }

    fn inner(&self) -> &BuiAppInner<StoreType, CallbackType> {
        &self.inner
    }
    // fn inner_mut(&mut self) -> &mut BuiAppInner<StoreType, CallbackType> {
    //     &mut self.inner
    // }
}

#[cfg(feature = "with_camtrig")]
struct SerialJoinHandles {
    serial_read_cjh: ControlledJoinHandle<()>,
    serial_write_cjh: ControlledJoinHandle<()>,
    serial_heartbeat_cjh: ControlledJoinHandle<()>,
}

#[cfg(feature = "with_camtrig")]
impl SerialJoinHandles {
    fn close_and_join_all(self) -> std::thread::Result<()> {
        self.serial_read_cjh.close_and_join()?;
        self.serial_write_cjh.close_and_join()?;
        self.serial_heartbeat_cjh.close_and_join()?;
        Ok(())
    }
    fn stoppers(&self) -> Vec<thread_control::Control> {
        vec![
            self.serial_read_cjh.control.clone(),
            self.serial_write_cjh.control.clone(),
            self.serial_heartbeat_cjh.control.clone(),
        ]
    }
}

#[cfg(not(feature = "with_camtrig"))]
struct SerialJoinHandles {}

#[cfg(not(feature = "with_camtrig"))]
impl SerialJoinHandles {
    fn close_and_join_all(self) -> std::thread::Result<()> {
        Ok(())
    }
    fn stoppers(&self) -> Vec<thread_control::Control> {
        vec![]
    }
}

#[cfg(feature = "fiducial")]
fn det2display(det: &apriltag::Detection) -> http_video_streaming_types::Point {
    let center = det.center();
    video_streaming::Point {
        x: center[0] as f32,
        y: center[1] as f32,
        theta: None,
        area: None,
    }
}

#[cfg(feature = "fiducial")]
fn frame2april(frame: &DynamicFrame) -> Option<apriltag::ImageU8Borrowed> {
    use machine_vision_formats::{ImageData, Stride};
    match frame {
        DynamicFrame::Mono8(frame) => Some(apriltag::ImageU8Borrowed::new(
            frame.width().try_into().unwrap(),
            frame.height().try_into().unwrap(),
            frame.stride().try_into().unwrap(),
            frame.image_data(),
        )),
        _ => None,
    }
}

#[cfg(feature = "with_camtrig")]
fn run_camtrig(
    shared_store_arc: Arc<RwLock<ChangeTracker<StoreType>>>,
    camtrig_tx_std: channellib::Sender<ToCamtrigDevice>,
    camtrig_rx: channellib::Receiver<ToCamtrigDevice>,
    camtrig_heartbeat_update_arc: Arc<RwLock<std::time::Instant>>,
    tx_cam_arg: mpsc::Sender<CamArg>,
) -> Result<SerialJoinHandles> {
    use camtrig::CamtrigCodec;
    use camtrig_comms::{ChannelState, DeviceState, OnState, Running, TriggerState};

    fn make_chan(num: u8, on_state: OnState) -> ChannelState {
        let intensity = camtrig_comms::MAX_INTENSITY;
        ChannelState {
            num,
            intensity,
            on_state,
        }
    }

    let first_camtrig_state = DeviceState {
        trig: TriggerState {
            running: Running::ConstantFreq(1),
        },
        ch1: make_chan(1, OnState::Off),
        ch2: make_chan(2, OnState::Off),
        ch3: make_chan(3, OnState::Off),
        ch4: make_chan(4, OnState::Off),
    };

    {
        let mut tracker = shared_store_arc.write();
        tracker.modify(|shared| shared.camtrig_device_state = Some(first_camtrig_state.clone()));
    }

    camtrig_tx_std
        .send(ToCamtrigDevice::DeviceState(first_camtrig_state))
        .cb_ok();

    let settings = serialport::SerialPortSettings {
        baud_rate: 9600,
        data_bits: serialport::DataBits::Eight,
        flow_control: serialport::FlowControl::None,
        parity: serialport::Parity::None,
        stop_bits: serialport::StopBits::One,
        timeout: std::time::Duration::from_millis(10_000),
    };

    let port = {
        let tracker = shared_store_arc.read();
        let shared = tracker.as_ref();

        match shared.camtrig_device_path {
            Some(ref serial_device) => {
                // open with default settings 9600 8N1
                serialport::open_with_settings(serial_device, &settings)?
            }
            None => {
                return Err(StrandCamError::StringError(
                    "no camtrig device path given".into(),
                ));
            }
        }
    };

    // separate reader and writer
    let mut reader_port = port.try_clone()?;
    let mut writer_port = port;

    let (flag, control) = thread_control::make_pair();
    let tx_cam_arg2 = tx_cam_arg.clone();
    let join_handle = std::thread::Builder::new()
        .name("serialport reader".to_string())
        .spawn(move || {
            // camtrig ignore for now

            let thread_closer = CloseAppOnThreadExit::new(tx_cam_arg2, file!(), line!());

            let mut codec = CamtrigCodec::new();
            let mut buf = bytes::BytesMut::with_capacity(1000);
            let mut read_buf = [0; 100];

            while flag.is_alive() {
                // blocking read from serial port
                match reader_port.read(&mut read_buf[..]) {
                    Ok(n_bytes) => {
                        buf.extend_from_slice(&read_buf[..n_bytes]);
                        use tokio_util::codec::Decoder;
                        if let Some(item) = thread_closer.check(codec.decode(&mut buf)) {
                            info!("read from camtrig device: {:?}", item);

                            {
                                // elsewhere check if this happens every CAMTRIG_HEARTBEAT_INTERVAL_MSEC or so.
                                let mut camtrig_heartbeat_update =
                                    camtrig_heartbeat_update_arc.write();
                                *camtrig_heartbeat_update = std::time::Instant::now();
                            }
                        }
                    }
                    Err(e) => match e.kind() {
                        std::io::ErrorKind::TimedOut => continue,
                        _ => {
                            thread_closer.fail(e.into());
                        }
                    },
                }
            }
            thread_closer.success();
        })?
        .into();
    let serial_read_cjh = ControlledJoinHandle {
        control,
        join_handle,
    };

    let (flag, control) = thread_control::make_pair();
    let tx_cam_arg2 = tx_cam_arg.clone();
    let join_handle = std::thread::Builder::new()
        .name("serialport writer".to_string())
        .spawn(move || {
            // camtrig ignore for now
            let thread_closer = CloseAppOnThreadExit::new(tx_cam_arg2, file!(), line!());
            let mut codec = CamtrigCodec::new();
            let mut buf = bytes::BytesMut::with_capacity(1000);

            while flag.is_alive() {
                let mut msgs = Vec::new();
                loop {
                    match camtrig_rx.try_recv() {
                        Ok(msg) => msgs.push(msg),
                        Err(e) => {
                            if e.is_empty() {
                                break;
                            } else {
                                thread_closer.fail(e.into());
                            }
                        }
                    }
                }

                let msg = match msgs.len() {
                    0 => thread_closer.check(camtrig_rx.recv()),
                    1 => msgs[0],
                    _ => {
                        error!(
                            "error: falling behind sending messages. dropping all but most \
                     recent. This is highly suboptimal and should be removed before using \
                     to perform experiments."
                        );
                        msgs[msgs.len() - 1]
                    }
                };

                if let ToCamtrigDevice::DeviceState(ref next_state) = msg {
                    // make an internal copy of state going to camtrig device
                    let mut tracker = shared_store_arc.write();
                    tracker.modify(|shared| {
                        shared.camtrig_device_state = Some(next_state.clone());
                    });
                }

                info!("sending message to camtrig device: {:?}", msg);
                use bytes::buf::Buf;
                use tokio_util::codec::Encoder;
                thread_closer.check(codec.encode(msg, &mut buf));
                let n_bytes = thread_closer.check(writer_port.write(&buf));
                buf.advance(n_bytes);
            }
            thread_closer.success();
        })?
        .into();
    let serial_write_cjh = ControlledJoinHandle {
        control,
        join_handle,
    };

    let (flag, control) = thread_control::make_pair();
    let join_handle = std::thread::Builder::new()
        .name("serialport timer".to_string())
        .spawn(move || {
            // camtrig ignore for now
            let thread_closer = CloseAppOnThreadExit::new(tx_cam_arg, file!(), line!());
            while flag.is_alive() {
                std::thread::sleep(std::time::Duration::from_millis(
                    CAMTRIG_HEARTBEAT_INTERVAL_MSEC,
                ));
                thread_closer.check(camtrig_tx_std.send(ToCamtrigDevice::TimerRequest));
            }
            thread_closer.success();
        })?
        .into();
    let serial_heartbeat_cjh = ControlledJoinHandle {
        control,
        join_handle,
    };

    Ok(SerialJoinHandles {
        serial_read_cjh,
        serial_write_cjh,
        serial_heartbeat_cjh,
    })
}

async fn check_version(
    client: hyper::Client<HttpsConnector<hyper::client::HttpConnector>>,
    known_version: Arc<RwLock<semver::Version>>,
) -> hyper::Result<()> {
    let url = format!("https://version-check.strawlab.org/{}", env!("APP_NAME"));
    let url = url.parse::<hyper::Uri>().unwrap();
    let agent = format!("{}/{}", env!("APP_NAME"), *known_version.read());

    let req = hyper::Request::builder()
        .uri(url)
        .header(hyper::header::USER_AGENT, agent.as_str())
        .body(hyper::body::Body::empty())
        .unwrap();

    #[derive(Debug, Deserialize, PartialEq, Clone)]
    struct VersionResponse {
        available: semver::Version,
        message: String,
    }

    let known_version2 = known_version.clone();

    let res = client.request(req).await?;

    if res.status() != hyper::StatusCode::OK {
        // should return error?
        return Ok(());
    }

    let known_version3 = known_version2.clone();
    let (_parts, body) = res.into_parts();

    // convert stream of Result<Chunk> into future of Vec<Result<Chunk>>
    let data_fut = body.fold(vec![], |mut buf, result_chunk| async {
        buf.push(result_chunk);
        buf
    });

    // now in this future handle the payload
    let vec_res_chunk: Vec<hyper::Result<hyper::body::Bytes>> = data_fut.await;

    // move error to outer type
    let res_vec_chunk: hyper::Result<Vec<hyper::body::Bytes>> = vec_res_chunk.into_iter().collect();

    let chunks = res_vec_chunk?;

    let data: Vec<u8> = chunks.into_iter().fold(vec![], |mut buf, chunk| {
        // trace!("got chunk: {}", String::from_utf8_lossy(&chunk));
        buf.extend_from_slice(&*chunk);
        buf
    });
    let version: VersionResponse = serde_json::from_slice(&data).unwrap();
    let mut known_v = known_version3.write();
    if version.available > *known_v {
        info!(
            "New version of {} is available: {}. {}",
            env!("APP_NAME"),
            version.available,
            version.message
        );
        *known_v = version.available;
    }

    Ok(())
}

fn get_mkv_writing_application(is_braid: bool) -> String {
    if is_braid {
        format!(
            "braid-{}-{}",
            env!("CARGO_PKG_NAME"),
            env!("CARGO_PKG_VERSION")
        )
    } else {
        format!("{}-{}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
    }
}

fn display_qr_url(url: &str) {
    use qrcodegen::{QrCode, QrCodeEcc};
    use std::io::stdout;

    let qr = QrCode::encode_text(&url, QrCodeEcc::Low).unwrap();

    let stdout = stdout();
    let mut stdout_handle = stdout.lock();
    writeln!(stdout_handle).expect("write failed");
    for y in 0..qr.size() {
        write!(stdout_handle, " ").expect("write failed");
        for x in 0..qr.size() {
            write!(
                stdout_handle,
                "{}",
                if qr.get_module(x, y) { "██" } else { "  " }
            )
            .expect("write failed");
        }
        writeln!(stdout_handle).expect("write failed");
    }
    writeln!(stdout_handle).expect("write failed");
}

#[cfg(feature = "image_tracker")]
/// Defines whether runtime changes from the user are persisted to disk.
///
/// If they are persisted to disk, upon program re-start, the disk
/// is checked and preferences are loaded from there. If they cannot
/// be loaded, the defaults are used.
pub enum ImPtDetectCfgSource {
    ChangesNotSavedToDisk(ImPtDetectCfg),
    ChangedSavedToDisk((&'static AppInfo, String)),
}

#[cfg(feature = "plugin-process-frame")]
pub struct ProcessFrameCbData {
    pub func_ptr: plugin_defs::ProcessFrameFunc,
    pub data_handle: plugin_defs::DataHandle,
}

// Ideally it would just be DataHandle which we declare Send, but we cannot do
// that because it is just a type alias of "*mut c_void" which is defined
// elsewhere.
#[cfg(feature = "plugin-process-frame")]
unsafe impl Send for ProcessFrameCbData {}

#[allow(dead_code)]
#[cfg(not(feature = "plugin-process-frame"))]
struct ProcessFrameCbData {}

#[derive(Debug, Serialize, Deserialize)]
struct MomentCentroid {
    x: f32,
    y: f32,
    center_x: u32,
    center_y: u32,
}

#[derive(Debug, Serialize, Deserialize)]
enum ToDevice {
    Centroid(MomentCentroid),
}

pub struct StrandCamArgs {
    /// Is Strand Cam running inside Braid context?
    pub is_braid: bool,
    pub secret: Option<Vec<u8>>,
    pub camera_name: Option<String>,
    pub pixel_format: Option<String>,
    pub http_server_addr: String,
    pub no_browser: bool,
    pub mkv_filename_template: String,
    pub fmf_filename_template: String,
    pub ufmf_filename_template: String,
    #[cfg(feature = "image_tracker")]
    pub tracker_cfg_src: ImPtDetectCfgSource,
    pub csv_save_dir: String,
    pub raise_grab_thread_priority: bool,
    #[cfg(feature = "posix_sched_fifo")]
    pub process_frame_priority: Option<(i32, i32)>,
    pub camtrig_device_path: Option<String>,
    pub use_cbor_packets: bool,
    pub ros_periodic_update_interval: std::time::Duration,
    #[cfg(feature = "debug-images")]
    pub debug_addr: std::net::SocketAddr,
    pub mainbrain_internal_addr: Option<MainbrainBuiLocation>,
    pub camdata_addr: Option<RealtimePointsDestAddr>,
    pub show_url: bool,
    #[cfg(feature = "plugin-process-frame")]
    pub process_frame_callback: Option<ProcessFrameCbData>,
    #[cfg(feature = "plugin-process-frame")]
    pub plugin_wait_dur: std::time::Duration,
    #[cfg(feature = "flydratrax")]
    pub save_empty_data2d: SaveEmptyData2dType,
    #[cfg(feature = "flydratrax")]
    pub model_server_addr: std::net::SocketAddr,
    #[cfg(feature = "flydratrax")]
    pub flydratrax_calibration_source: CalSource,
    #[cfg(feature = "fiducial")]
    pub apriltag_csv_filename_template: String,

    /// If set, camera acquisition will external trigger.
    pub force_camera_sync_mode: bool,

    /// If not Enable, limit framerate (FPS) at startup.
    pub software_limit_framerate: StartSoftwareFrameRateLimit,
}

pub type SaveEmptyData2dType = bool;

pub enum CalSource {
    /// Use circular tracking region to create calibration
    PseudoCal,
    /// Use flydra .xml file with single camera for calibration
    XmlFile(std::path::PathBuf),
    /// Use pymvg .json file with single camera for calibration
    PymvgJsonFile(std::path::PathBuf),
}

#[derive(Clone)]
pub enum StartSoftwareFrameRateLimit {
    /// Set the frame_rate limit at a given frame rate.
    Enable(f64),
    /// Disable the frame_rate limit.
    Disabled,
    /// Do not change the frame rate limit.
    NoChange,
}

impl Default for StrandCamArgs {
    fn default() -> Self {
        Self {
            is_braid: false,
            secret: None,
            camera_name: None,
            pixel_format: None,
            http_server_addr: "127.0.0.1:0".to_string(),
            no_browser: true,
            mkv_filename_template: "movie%Y%m%d_%H%M%S.%f.mkv".to_string(),
            fmf_filename_template: "movie%Y%m%d_%H%M%S.fmf".to_string(),
            ufmf_filename_template: "movie%Y%m%d_%H%M%S.ufmf".to_string(),
            #[cfg(feature = "fiducial")]
            apriltag_csv_filename_template: strand_cam_storetype::APRILTAG_CSV_TEMPLATE_DEFAULT
                .to_string(),
            #[cfg(feature = "image_tracker")]
            tracker_cfg_src: ImPtDetectCfgSource::ChangesNotSavedToDisk(default_im_pt_detect()),
            csv_save_dir: "/dev/null".to_string(),
            raise_grab_thread_priority: false,
            #[cfg(feature = "posix_sched_fifo")]
            process_frame_priority: None,
            camtrig_device_path: None,
            use_cbor_packets: true,
            ros_periodic_update_interval: std::time::Duration::from_millis(4500),
            #[cfg(feature = "debug-images")]
            debug_addr: std::str::FromStr::from_str(DEBUG_ADDR_DEFAULT).unwrap(),
            mainbrain_internal_addr: None,
            camdata_addr: None,
            show_url: true,
            #[cfg(feature = "plugin-process-frame")]
            process_frame_callback: None,
            #[cfg(feature = "plugin-process-frame")]
            plugin_wait_dur: std::time::Duration::from_millis(5),
            force_camera_sync_mode: false,
            software_limit_framerate: StartSoftwareFrameRateLimit::NoChange,
            #[cfg(feature = "flydratrax")]
            flydratrax_calibration_source: CalSource::PseudoCal,
            #[cfg(feature = "flydratrax")]
            save_empty_data2d: true,
            #[cfg(feature = "flydratrax")]
            model_server_addr: flydra_types::DEFAULT_MODEL_SERVER_ADDR.parse().unwrap(),
        }
    }
}

pub fn run_app(args: StrandCamArgs) -> std::result::Result<(), anyhow::Error> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(4)
        .thread_name("strand-cam-runtime")
        .thread_stack_size(3 * 1024 * 1024)
        .build()?;

    let my_handle = runtime.handle().clone();

    let my_runtime = Arc::new(runtime);
    let (_bui_server_info, tx_cam_arg2, fut, _my_app) =
        my_runtime.block_on(setup_app(my_handle, args))?;

    ctrlc::set_handler(move || {
        info!("got Ctrl-C, shutting down");
        let mut tx_cam_arg = tx_cam_arg2.clone();

        // Send quit message.
        debug!("starting to send quit message {}:{}", file!(), line!());
        match futures::executor::block_on(tx_cam_arg.send(CamArg::DoQuit)) {
            Ok(()) => {}
            Err(e) => {
                error!("failed sending quit command: {}", e);
            }
        }
        debug!("done sending quit message {}:{}", file!(), line!());
    })
    .expect("Error setting Ctrl-C handler");

    my_runtime.block_on(fut);

    info!("done");
    Ok(())
}

// We want to remove rustfmt::skip attribute. There is a bug similar to
// https://github.com/rust-lang/rustfmt/issues/4109 which prevents this. Bug
// 4109 does not seem exactly correct (at least presuming this was fixed in
// rustfmt 1.4.24-stable (eb894d53 2020-11-05)), but I have not found the
// correct bug.
#[rustfmt::skip]
pub async fn setup_app(
    rt_handle: tokio::runtime::Handle,
    args: StrandCamArgs)
    -> anyhow::Result<(BuiServerInfo, mpsc::Sender<CamArg>, impl futures::Future<Output=()>, NoisyDrop<MyApp>)>
{
    debug!("CLI request for camera {:?}", args.camera_name);

    // -----------------------------------------------

    let sync_mod = backend::new_module()?;
    let mut mymod = ci2_async::into_threaded_async(sync_mod);

    info!("camera module: {}", mymod.name());

    let cam_infos = mymod.camera_infos()?;
    if cam_infos.len() == 0 {
        return Err(StrandCamError::NoCamerasFound.into());
    }

    for cam_info in cam_infos.iter() {
        info!("  camera {:?} detected", cam_info.name());
    }

    let name = match args.camera_name {
        Some(ref name) => name,
        None => cam_infos[0].name(),
    };

    let mut cam = match mymod.threaded_async_camera(&name) {
        Ok(cam) => cam,
        Err(e) => {
            let msg = format!("{}",e);
            error!("{}", msg);
            if msg.contains("Device is exclusively opened by another client") {
                if !args.no_browser {
                    let url = format!("http://{}", &args.http_server_addr);
                    open_browser(url)?;
                    // Sleep to prevent process exit before browser open.
                    std::thread::sleep(std::time::Duration::from_millis(10000));
                } else {
                    info!("not opening browser");
                }
            }
            return Err(e.into());
        }
    };

    let raw_name = cam.name().to_string();
    info!("  got camera {}", raw_name);
    let cam_name = RawCamName::new(raw_name);

    for pixfmt in cam.possible_pixel_formats()?.iter() {
        debug!("  possible pixel format: {}", pixfmt);
    }

    if let Some(ref pixfmt_str) = args.pixel_format {
        use std::str::FromStr;
        let pixfmt = PixFmt::from_str(&pixfmt_str)
            .map_err(|e: &str| StrandCamError::StringError(e.to_string()))?;
        info!("  setting pixel format: {}", pixfmt);
        cam.set_pixel_format(pixfmt)?;
    }

    debug!("  current pixel format: {}", cam.pixel_format()?);

    let (frame_rate_limit_supported, mut frame_rate_limit_enabled) = {
        // This entire section should be removed and converted to a query
        // of the cameras capabilities.

        // Save the value of whether the frame rate limiter is enabled.
        let frame_rate_limit_enabled = cam.acquisition_frame_rate_enable()?;
        debug!("frame_rate_limit_enabled {}", frame_rate_limit_enabled);

        // Check if we can set the frame rate, first by setting a limit to be on.
        let frame_rate_limit_supported = match cam.set_acquisition_frame_rate_enable(true) {
            Ok(()) => {
                debug!("set set_acquisition_frame_rate_enable true");
                // Then by setting a limit to be off.
                match cam.set_acquisition_frame_rate_enable(false) {
                    Ok(()) => {debug!("{}:{}",file!(),line!());true},
                    Err(e) => {debug!("err {} {}:{}",e, file!(),line!());false},
                }
            },
            Err(e) => {debug!("err {} {}:{}",e,file!(),line!());false},
        };

        if frame_rate_limit_supported {
            // Restore the state of the frame rate limiter.
            cam.set_acquisition_frame_rate_enable(frame_rate_limit_enabled)?;
            debug!("set frame_rate_limit_enabled {}", frame_rate_limit_enabled);
        }

        (frame_rate_limit_supported, frame_rate_limit_enabled)
    };

    cam.set_acquisition_mode(ci2::AcquisitionMode::Continuous)?;
    cam.acquisition_start()?;
    // Buffer 20 frames to be processed before dropping them.
    let (tx_frame, rx_frame) = channellib::bounded::<Msg>(20);
    let tx_frame2 = tx_frame.clone();
    let tx_frame3 = tx_frame.clone();

    // Get initial frame to determine width, height and pixel_format.
    debug!("  started acquisition, waiting for first frame");
    let frame = cam.next_frame()?;
    info!("  acquired first frame: {}x{}", frame.width(), frame.height());

    #[allow(unused_variables)]
    let (plugin_handler_thread_tx, plugin_handler_thread_rx) = channellib::bounded::<DynamicFrame>(500);
    #[allow(unused_variables)]
    let (plugin_result_tx, plugin_result_rx) = channellib::bounded::<_>(500);

    #[cfg(feature="plugin-process-frame")]
    let plugin_wait_dur = args.plugin_wait_dur;

    #[cfg(not(feature="plugin-process-frame"))]
    let plugin_wait_dur = std::time::Duration::from_millis(5);

    let (firehose_tx, firehose_rx) = channellib::bounded::<AnnotatedFrame>(5);

    let image_width = frame.width();
    let image_height = frame.height();

    #[cfg(feature = "posix_sched_fifo")]
    let process_frame_priority = args.process_frame_priority;

    #[cfg(not(feature = "posix_sched_fifo"))]
    let process_frame_priority = None;

    let raise_grab_thread_priority = args.raise_grab_thread_priority;

    #[cfg(feature = "debug-images")]
    let debug_addr = args.debug_addr;
    let ros_periodic_update_interval = args.ros_periodic_update_interval;
    #[cfg(feature="image_tracker")]
    let tracker_cfg_src = args.tracker_cfg_src;

    #[cfg(feature="flydratrax")]
    let save_empty_data2d = args.save_empty_data2d;

    #[cfg(not(feature="flydratrax"))]
    let save_empty_data2d = true; // not used

    #[cfg(feature="image_tracker")]
    let tracker_cfg = match &tracker_cfg_src {
        &ImPtDetectCfgSource::ChangedSavedToDisk(ref src) => {
            // Retrieve the saved preferences
            let (ref app_info, ref prefs_key) = src;
            match ImPtDetectCfg::load(app_info, prefs_key) {
                Ok(cfg) => cfg,
                Err(e) => {
                    info!("Failed loading image detection config ({}), using defaults.", e);
                    default_im_pt_detect()
                }
            }
        },
        &ImPtDetectCfgSource::ChangesNotSavedToDisk(ref cfg) => {
            cfg.clone()
        }
    };

    #[cfg(feature="image_tracker")]
    let im_pt_detect_cfg = tracker_cfg.clone();

    let mainbrain_internal_addr = args.mainbrain_internal_addr.clone();

    let (cam_args_tx, mut cam_args_rx) = mpsc::channel(100);
    let (camtrig_tx_std, camtrig_rx) = channellib::unbounded();

    let camtrig_heartbeat_update_arc = Arc::new(RwLock::new(std::time::Instant::now()));

    let gain_ranged = RangedValue {
        name: "gain".into(),
        unit: "dB".into(),
        min: cam.gain_range()?.0,
        max: cam.gain_range()?.1,
        current: cam.gain()?,
    };
    let exposure_ranged = RangedValue {
        name: "exposure time".into(),
        unit: "μsec".into(),
        min: cam.exposure_time_range()?.0,
        max: cam.exposure_time_range()?.1,
        current: cam.exposure_time()?,
    };
    let gain_auto = cam.gain_auto().ok();
    let exposure_auto = cam.exposure_auto().ok();

    let mut frame_rate_limit = if frame_rate_limit_supported {
        let (min, max) = cam.acquisition_frame_rate_range()?;
        Some(RangedValue {
            name: "frame rate".into(),
            unit: "Hz".into(),
            min,
            max,
            current: cam.acquisition_frame_rate()?,
        })
    } else {
        None
    };

    if args.force_camera_sync_mode {
        // The trigger selector must be set before the trigger mode.
        cam.set_trigger_selector(ci2_types::TriggerSelector::FrameStart).unwrap();
        cam.set_trigger_mode(ci2::TriggerMode::On).unwrap();
    }

    if let StartSoftwareFrameRateLimit::Enable(fps_limit) = &args.software_limit_framerate {
        // Set the camera.
        cam.set_acquisition_frame_rate(*fps_limit).unwrap();
        cam.set_acquisition_frame_rate_enable(true).unwrap();
        // Store the values we set.
        if let Some(ref mut ranged) = frame_rate_limit {
            ranged.current = cam.acquisition_frame_rate()?;
        } else {
            panic!("cannot set software frame rate limit");
        }
        frame_rate_limit_enabled = cam.acquisition_frame_rate_enable()?;
    }

    let trigger_mode = cam.trigger_mode()?;
    let trigger_selector = cam.trigger_selector()?;
    debug!("  got camera values");

    let camera_cfg = CameraCfgFview2_0_26 {
        vendor: cam.vendor().into(),
        model: cam.model().into(),
        serial: cam.serial().into(),
        width: cam.width()?,
        height: cam.height()?,
    };

    #[cfg(feature="flydratrax")]
    let kalman_tracking_config = {
        if let ImPtDetectCfgSource::ChangedSavedToDisk(ref src) = tracker_cfg_src {
            // Retrieve the saved preferences
            let (ref app_info, ref _im_pt_detect_prefs_key) = src;
            match KalmanTrackingConfig::load(app_info, KALMAN_TRACKING_PREFS_KEY) {
                Ok(cfg) => cfg,
                Err(e) => {
                    info!("Failed loading kalman tracking config ({}), using defaults.", e);
                    KalmanTrackingConfig::default()
                }
            }
        } else {
            panic!("flydratrax requires saving changes to disk");
        }
    };

    #[cfg(feature="flydratrax")]
    let led_program_config = {
        if let ImPtDetectCfgSource::ChangedSavedToDisk(ref src) = tracker_cfg_src {
            // Retrieve the saved preferences
            let (ref app_info, ref _im_pt_detect_prefs_key) = src;
            match LedProgramConfig::load(app_info, LED_PROGRAM_PREFS_KEY) {
                Ok(cfg) => cfg,
                Err(e) => {
                    info!("Failed loading LED config ({}), using defaults.", e);
                    LedProgramConfig::default()
                }
            }
        } else {
            panic!("flydratrax requires saving changes to disk");
        }
    };

    let cuda_devices = match nvenc::Dynlibs::new() {
        Ok(libs) => {
            match nvenc::NvEnc::new(&libs) {
                Ok(nv_enc) => {
                    let n = nv_enc.cuda_device_count()?;
                    let r: Result<Vec<String>> = (0..n).map(|i| {
                        let dev = nv_enc.new_cuda_device(i)?;
                        Ok(dev.name().map_err(|e| nvenc::NvEncError::from(e))?)
                    }).collect();
                    r?
                }
                Err(e) => {
                    info!("CUDA and nvidia-encode libraries loaded, but \
                        error during initialization: {}", e);
                    // empty vector
                    Vec::new()
                },
            }
        }
        Err(e) => {
            // no cuda library, no libs
            info!("CUDA and nvidia-encode libraries not loaded: {}", e);
            // empty vector
            Vec::new()
        }
    };

    #[cfg(not(feature="fiducial"))]
    let apriltag_state = None;

    #[cfg(feature="fiducial")]
    let apriltag_state = Some(ApriltagState::default());

    let im_ops_state = ImOpsState::default();

    #[cfg(not(feature="fiducial"))]
    let format_str_apriltag_csv = "".into();

    #[cfg(feature="fiducial")]
    let format_str_apriltag_csv = args.apriltag_csv_filename_template.into();

    // Here we just create some default, it does not matter what, because it
    // will not be used for anything.
    #[cfg(not(feature="image_tracker"))]
    let im_pt_detect_cfg = im_pt_detect_config::default_absdiff();

    #[cfg(feature="image_tracker")]
    let has_image_tracker_compiled = true;

    #[cfg(not(feature="image_tracker"))]
    let has_image_tracker_compiled = false;

    let mut mkv_recording_config = MkvRecordingConfig::default();

    let is_braid = args.is_braid;
    mkv_recording_config.writing_application = Some(get_mkv_writing_application(is_braid));

    let shared_store = ChangeTracker::new(StoreType {
        is_braid,
        is_recording_mkv: None,
        is_recording_fmf: None,
        is_recording_ufmf: None,
        format_str_apriltag_csv,
        format_str_mkv: args.mkv_filename_template.into(),
        format_str: args.fmf_filename_template.into(),
        format_str_ufmf: args.ufmf_filename_template.into(),
        camera_name: cam.name().into(),
        recording_filename: None,
        recording_framerate: RecordingFrameRate::default(),
        mkv_recording_config,
        gain: gain_ranged,
        gain_auto: gain_auto,
        exposure_time: exposure_ranged,
        exposure_auto: exposure_auto,
        frame_rate_limit_enabled,
        frame_rate_limit,
        trigger_mode: trigger_mode,
        trigger_selector: trigger_selector,
        image_width: image_width,
        image_height: image_height,
        is_doing_object_detection: false,
        measured_fps: 0.0,
        is_saving_im_pt_detect_csv: None,
        has_image_tracker_compiled,
        im_pt_detect_cfg,
        #[cfg(feature="flydratrax")]
        kalman_tracking_config,
        #[cfg(feature="flydratrax")]
        led_program_config,
        #[cfg(feature="with_camtrig")]
        camtrig_device_lost: false,
        camtrig_device_state: None,
        camtrig_device_path: args.camtrig_device_path,
        #[cfg(feature="checkercal")]
        checkerboard_data: strand_cam_storetype::CheckerboardCalState::new(),
        #[cfg(feature="checkercal")]
        checkerboard_save_debug: None,
        post_trigger_buffer_size: 0,
        cuda_devices,
        apriltag_state,
        im_ops_state,
        had_frame_processing_error: false,
    });

    let frame_processing_error_state = Arc::new(RwLock::new(FrameProcessingErrorState::default()));

    let (flag, control) = thread_control::make_pair();
    let use_cbor_packets = args.use_cbor_packets;
    let camdata_addr = args.camdata_addr;

    let mut config = get_default_config();
    config.cookie_name = "strand-camclient".to_string();

    let shared_store_arc = Arc::new(RwLock::new(shared_store));

    let cam_args_tx2 = cam_args_tx.clone();
    let secret = args.secret.clone();

    let (quit_trigger, valve) = stream_cancel::Valve::new();

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    #[cfg(feature="flydratrax")]
    let (model_server_shutdown_tx, model_server_shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    #[cfg(feature="debug-images")]
    let (debug_image_shutdown_tx, debug_image_shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    let (firehose_callback_rx, my_app) =
    MyApp::new(
        rt_handle.clone(),
        shared_store_arc.clone(),
        secret,
        &args.http_server_addr,
        config,
        cam_args_tx2.clone(),
        camtrig_tx_std.clone(),
        tx_frame3,
        valve.clone(),
        shutdown_rx).await?;

    let my_app = noisy_drop!(my_app);

    // The value `args.http_server_addr` is transformed to
    // `local_addr` by doing things like replacing port 0
    // with the actual open port number.

    let (is_loopback, http_camserver_info) = {
        let local_addr = my_app.inner().local_addr().clone();
        let is_loopback = local_addr.ip().is_loopback();
        let token = my_app.inner().token();
        (is_loopback, BuiServerInfo::new(local_addr, token))
    };

    let url = http_camserver_info.guess_base_url_with_token();

    if args.show_url {
        println!("Depending on things, you may be able to login with this url: {}",
            url);

        if !is_loopback {
            println!("This same URL as a QR code:");
            display_qr_url(&url);
        }
    }

    #[cfg(feature="plugin-process-frame")]
    let do_process_frame_callback = args.process_frame_callback.is_some();

    #[cfg(feature="plugin-process-frame")]
    let process_frame_callback = args.process_frame_callback;

    #[cfg(not(feature="plugin-process-frame"))]
    let do_process_frame_callback = false;

    let collected_corners_arc = Arc::new(RwLock::new(Vec::new()));
    let collected_corners_arc2 = collected_corners_arc.clone();

    #[cfg(feature="checkercal")]
    let cam_name2 = cam_name.clone();

    let frame_process_cjh = {
        let pixel_format = frame.pixel_format();
        let is_starting = Arc::new(true);
        let is_starting_weak = Arc::downgrade(&is_starting);

        let csv_save_dir = args.csv_save_dir.clone();
        #[cfg(feature="flydratrax")]
        let model_server_addr = args.model_server_addr.clone();
        let camtrig_tx_std = camtrig_tx_std.clone();
        let http_camserver_info2 = http_camserver_info.clone();
        let camtrig_heartbeat_update_arc2 = camtrig_heartbeat_update_arc.clone();
        let cam_args_tx2 = cam_args_tx.clone();

        let handle2 = rt_handle.clone();
        #[cfg(feature="flydratrax")]
        let (model_server, flydratrax_calibration_source) = {

            let model_server_shutdown_rx = Some(model_server_shutdown_rx);

            info!("send_pose server at {}", model_server_addr);
            let info = flydra_types::StaticMainbrainInfo {
                name: env!("CARGO_PKG_NAME").into(),
                version: env!("CARGO_PKG_VERSION").into(),
            };

            // we need the tokio reactor already by here
            let model_server = flydra2::new_model_server(valve.clone(), model_server_shutdown_rx, &model_server_addr, info, handle2.clone()).await?;
            let flydratrax_calibration_source = args.flydratrax_calibration_source;
            (model_server, flydratrax_calibration_source)
        };

        let valve2 = valve.clone();
        let frame_process_jh = std::thread::Builder::new().name("frame_process_thread".to_string()).spawn(move || { // confirmed closes
            let thread_closer = CloseAppOnThreadExit::new(cam_args_tx2.clone(), file!(), line!());
            thread_closer.maybe_err(frame_process_thread(
                    handle2,
                    #[cfg(feature="flydratrax")]
                    model_server,
                    #[cfg(feature="flydratrax")]
                    flydratrax_calibration_source,
                    cam_name,
                    camera_cfg,
                    image_width,
                    image_height,
                    pixel_format,
                    rx_frame,
                    cam_args_tx2,
                    #[cfg(feature="image_tracker")]
                    tracker_cfg,
                    std::path::Path::new(&csv_save_dir).to_path_buf(),
                    firehose_tx,
                    plugin_handler_thread_tx,
                    plugin_result_rx,
                    plugin_wait_dur,
                    camtrig_tx_std,
                    flag,
                    is_starting,
                    http_camserver_info2,
                    use_cbor_packets,
                    process_frame_priority,
                    ros_periodic_update_interval,
                    #[cfg(feature = "debug-images")]
                    debug_addr,
                    mainbrain_internal_addr,
                    camdata_addr,
                    camtrig_heartbeat_update_arc2,
                    do_process_frame_callback,
                    collected_corners_arc2,
                    save_empty_data2d,
                    valve2,
                    #[cfg(feature = "debug-images")]
                    Some(debug_image_shutdown_rx),
                ));
        })?;
        debug!("waiting for frame acquisition thread to start");
        loop {
            trace!("inner waiting for frame acquisition thread to start");
            match is_starting_weak.upgrade() {
                Some(_) => {},
                None => {
                    break;
                },
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        if control.is_done() {
            return Err(StrandCamError::ThreadDone.into());
        }
        ControlledJoinHandle {
            control,
            join_handle: frame_process_jh.into(),
        }
    };
    debug!("frame_process_thread spawned");

    tx_frame.send(Msg::Store(shared_store_arc.clone())).cb_ok();

    debug!("installing frame stream handler");

    #[cfg(feature="posix_sched_fifo")]
    fn with_priority() {
        // This function is run in the camera capture thread as it is started.
        let pid = 0; // this thread
        let priority = 99;
        match posix_scheduler::sched_setscheduler( pid,
            posix_scheduler::SCHED_FIFO, priority)
        {
            Ok(()) => info!("grabbing frames with SCHED_FIFO scheduler policy"),
            Err(e) => error!("failed to start frame grabber thread with \
                            SCHED_FIFO scheduler policy: {}", e),
        };
    }

    #[cfg(not(feature="posix_sched_fifo"))]
    fn with_priority() {
        // This funciton is run in the camera capture thread as it is started.
        debug!("starting async capture");
    }

    fn no_priority() {
        // This funciton is run in the camera capture thread as it is started.
        debug!("starting async capture");
    }

    let async_thread_start = if raise_grab_thread_priority {
        with_priority
    } else {
        no_priority
    };

    // install frame handling
    let n_buffered_frames = 100;
    let frame_stream = cam.frames(n_buffered_frames, async_thread_start)?;
    let mut frame_valved = valve.wrap(frame_stream);
    let cam_stream_future = {
        let shared_store_arc = shared_store_arc.clone();
        let frame_processing_error_state = frame_processing_error_state.clone();
        async move {
        while let Some(frame_msg) = frame_valved.next().await {
            match frame_msg {
                ci2_async::FrameResult::Frame(frame) => {
                    let frame: DynamicFrame = frame;
                    trace!(
                        "  got frame {}: {}x{}",
                        frame.extra().host_framenumber(),
                        frame.width(),
                        frame.height()
                    );
                    if tx_frame.is_full() {
                        let mut tracker = shared_store_arc.write();
                        tracker.modify(|tracker| {
                            let mut state = frame_processing_error_state.write();
                            {
                                match &*state {
                                    FrameProcessingErrorState::IgnoreAll => {},
                                    FrameProcessingErrorState::IgnoreUntil(ignore_until) => {
                                        let now = chrono::Utc::now();
                                        if now >= *ignore_until {
                                            tracker.had_frame_processing_error = true;
                                            *state = FrameProcessingErrorState::NotifyAll;
                                        }
                                    },
                                    FrameProcessingErrorState::NotifyAll => {
                                        tracker.had_frame_processing_error = true;
                                    },
                                }

                            }
                        });
                        error!("Channel full sending frame to process thread. Dropping frame data.");
                    } else {
                        tx_frame.send(Msg::Mframe(frame)).cb_ok();
                    }
                },
                ci2_async::FrameResult::SingleFrameError(s) => {
                    error!("SingleFrameError({})", s);
                },
            }
        };
        debug!("cam_stream_future future done {}:{}", file!(), line!());
    }};

    let do_version_check = match std::env::var_os("DISABLE_VERSION_CHECK") {
        Some(v) => if &v != "0" { false } else { true },
        None => { true },
    };

    // This is quick-and-dirtry version checking. It can be cleaned up substantially...
    if do_version_check {

        let app_version: semver::Version = {
            let mut my_version = semver::Version::parse(env!("CARGO_PKG_VERSION")).unwrap();
            my_version.build = semver::BuildMetadata::new(env!("GIT_HASH").to_string().as_str())?;
            my_version
        };

        info!("Welcome to {} {}. For more details \
            contact Andrew Straw <straw@bio.uni-freiburg.de>. This program will check for new \
            versions automatically. To disable printing this message and checking for new \
            versions, set the environment variable DISABLE_VERSION_CHECK=1.", env!("APP_NAME"),
            app_version);

        // TODO I just used Arc and RwLock to code this quickly. Convert to single-threaded
        // versions later.
        let known_version = Arc::new(RwLock::new(app_version));

        // Create a stream to call our closure now and every 30 minutes.
        let interval_stream = tokio::time::interval(
            std::time::Duration::from_secs(1800));

        let interval_stream = tokio_stream::wrappers::IntervalStream::new(interval_stream);

        let mut incoming1 = valve.wrap(interval_stream);

        let known_version2 = known_version.clone();
        let stream_future = async move {
            while let Some(_) = incoming1.next().await {
                let https = HttpsConnector::new();
                let client = hyper::Client::builder()
                    .build::<_, hyper::Body>(https);

                let r = check_version(client, known_version2.clone()).await;
                match r {
                    Ok(()) => {}
                    Err(e) => {error!("error checking version: {}",e);}
                }
            }
            debug!("version check future done {}:{}", file!(), line!());
        };
        rt_handle.spawn(Box::pin(stream_future)); // confirmed: valved and finishes
        debug!("version check future spawned {}:{}", file!(), line!());
    }

    rt_handle.spawn(Box::pin(cam_stream_future)); // confirmed: valved and finishes
    debug!("cam_stream_future future spawned {}:{}", file!(), line!());

    let cam_arg_future = {
        let shared_store_arc = shared_store_arc.clone();
        async move {
        // We do not put cam_args_rx behind a stream_cancel::Valve because
        // it is the top-level controller for quitting everything - if
        // a DoQuit message is received, then this while loop will end
        // and all the cleanup below will fire. It is done this way because
        // we need to be able to quit Strand Cam as a standalone program in
        // which case it catches its own Ctrl-C and then fires a DoQuit message,
        // or if it is run within Braid, in which Braid will send it a DoQuit
        // message. Finally, when other threads panic, they should also send a
        // DoQuit message.
        while let Some(cam_args) = cam_args_rx.next().await {
            debug!("handling camera command {:?}", cam_args);
            #[allow(unused_variables)]
            match cam_args {
                CamArg::SetIngoreFutureFrameProcessingErrors(v) => {
                    let mut state = frame_processing_error_state.write();
                    match v {
                        None => {
                            *state = FrameProcessingErrorState::IgnoreAll;
                        }
                        Some(val) => {
                            if val <= 0 {
                                *state = FrameProcessingErrorState::NotifyAll;
                            } else {
                                let when = chrono::Utc::now() + chrono::Duration::seconds(val);
                                *state = FrameProcessingErrorState::IgnoreUntil(when);
                            }
                        }
                    }

                    let mut tracker = shared_store_arc.write();
                    tracker.modify(|tracker| tracker.had_frame_processing_error = false);

                }
                CamArg::SetExposureTime(v) => {
                    match cam.set_exposure_time(v) {
                        Ok(()) => {
                            let mut tracker = shared_store_arc.write();
                            tracker.modify(|tracker| tracker.exposure_time.current = v);
                        }
                        Err(e) => {
                            error!("setting exposure_time: {:?}", e);
                        }
                    }
                }
                CamArg::SetGain(v) => {
                    match cam.set_gain(v) {
                        Ok(()) => {
                            let mut tracker = shared_store_arc.write();
                            tracker.modify(|tracker| tracker.gain.current = v);
                        }
                        Err(e) => {
                            error!("setting gain: {:?}", e);
                        }
                    }
                }
                CamArg::SetGainAuto(v) => {
                    match cam.set_gain_auto(v) {
                        Ok(()) => {
                            let mut tracker = shared_store_arc.write();
                            tracker.modify(|shared| {
                                match cam.gain_auto() {
                                    Ok(latest) => {
                                        shared.gain_auto = Some(latest);
                                    },
                                    Err(e) => {
                                        shared.gain_auto = Some(v);
                                        error!("after setting gain_auto, error getting: {:?}",e);
                                    }
                                }
                            });

                        }
                        Err(e) => {
                            error!("setting gain_auto: {:?}", e);
                        }
                    }
                }
                CamArg::SetRecordingFps(v) => {
                    let mut tracker = shared_store_arc.write();
                    tracker.modify(|tracker| tracker.recording_framerate = v);
                }
                CamArg::SetMkvRecordingConfig(mut cfg) => {
                    if cfg.writing_application.is_none() {
                        // The writing application is not set in the web UI
                        cfg.writing_application = Some(get_mkv_writing_application(is_braid));
                    }
                    let mut tracker = shared_store_arc.write();
                    tracker.modify(|tracker| tracker.mkv_recording_config = cfg);
                }
                CamArg::SetMkvRecordingFps(v) => {
                    let mut tracker = shared_store_arc.write();
                    tracker.modify(|tracker| tracker.mkv_recording_config.max_framerate = v);
                }
                CamArg::SetExposureAuto(v) => {
                    match cam.set_exposure_auto(v) {
                        Ok(()) => {
                            let mut tracker = shared_store_arc.write();
                            tracker.modify(|shared| {
                                match cam.exposure_auto() {
                                    Ok(latest) => {
                                        shared.exposure_auto = Some(latest);
                                    },
                                    Err(e) => {
                                        shared.exposure_auto = Some(v);
                                        error!("after setting exposure_auto, error getting: {:?}",e);
                                    }
                                }
                            });
                        }
                        Err(e) => {
                            error!("setting exposure_auto: {:?}", e);
                        }
                    }
                }
                CamArg::SetFrameRateLimitEnabled(v) => {
                    match cam.set_acquisition_frame_rate_enable(v) {
                        Ok(()) => {
                            let mut tracker = shared_store_arc.write();
                            tracker.modify(|shared| {
                                match cam.acquisition_frame_rate_enable() {
                                    Ok(latest) => {
                                        shared.frame_rate_limit_enabled = latest;
                                    },
                                    Err(e) => {
                                        error!("after setting frame_rate_limit_enabled, error getting: {:?}",e);
                                    }
                                }
                            });
                        }
                        Err(e) => {
                            error!("setting frame_rate_limit_enabled: {:?}", e);
                        }
                    }
                }
                CamArg::SetFrameRateLimit(v) => {
                    match cam.set_acquisition_frame_rate(v) {
                        Ok(()) => {
                            let mut tracker = shared_store_arc.write();
                            tracker.modify(|shared| {
                                match cam.acquisition_frame_rate() {
                                    Ok(latest) => {
                                        if let Some(ref mut frl) = shared.frame_rate_limit {
                                            frl.current = latest;
                                        } else {
                                            error!("frame_rate_limit is expectedly None");
                                        }
                                    },
                                    Err(e) => {
                                        error!("after setting frame_rate_limit, error getting: {:?}",e);
                                    }
                                }
                            });
                        }
                        Err(e) => {
                            error!("setting frame_rate_limit: {:?}", e);
                        }
                    }
                }

                CamArg::SetTriggerMode(v) => {
                    match cam.set_trigger_mode(v) {
                        Ok(()) => {
                            let mut tracker = shared_store_arc.write();
                            tracker.modify(|tracker| tracker.trigger_mode = v);
                        }
                        Err(e) => {
                            error!("setting trigger_mode: {:?}", e);
                        }
                    }
                }
                CamArg::SetTriggerSelector(v) => {
                    match cam.set_trigger_selector(v) {
                        Ok(()) => {
                            let mut tracker = shared_store_arc.write();
                            tracker.modify(|tracker| tracker.trigger_selector = v);
                        }
                        Err(e) => {
                            error!("setting trigger_selector: {:?}", e);
                        }
                    }
                }
                CamArg::SetFrameOffset(fo) => {
                    tx_frame2.send(Msg::SetFrameOffset(fo)).cb_ok();
                }
                CamArg::SetClockModel(cm) => {
                    tx_frame2.send(Msg::SetClockModel(cm)).cb_ok();
                }
                CamArg::SetFormatStr(v) => {
                    let mut tracker = shared_store_arc.write();
                    tracker.modify(|tracker| tracker.format_str = v);
                }
                CamArg::SetIsRecordingMkv(do_recording) => {
                    let mut tracker = shared_store_arc.write();
                    tracker.modify(|shared| {
                        let mkv_recording_config = shared.mkv_recording_config.clone();
                        if shared.is_recording_mkv.is_some() != do_recording {
                            info!("changed recording mkv value: do_recording={}", do_recording);
                            let new_val = if do_recording {
                                tx_frame2.send(Msg::StartMkv((shared.format_str_mkv.clone(), mkv_recording_config))).cb_ok();
                                // Some(RecordingPath::new(filename))
                                Some(RecordingPath::new(shared.format_str_mkv.clone()))
                            } else {
                                tx_frame2.send(Msg::StopMkv).cb_ok();
                                None
                            };
                            shared.is_recording_mkv = new_val;
                        }
                    });
                }
                CamArg::ToggleAprilTagFamily(family) => {
                    let mut tracker = shared_store_arc.write();
                    tracker.modify(|shared| {
                        if let Some(ref mut ts) = shared.apriltag_state {
                            if ts.is_recording_csv.is_some() {
                                error!("will not change families while recording CSV");
                            } else {
                                ts.april_family = family;
                            }
                        } else {
                            error!("no apriltag support, not switching state");
                        }
                    });
                }
                CamArg::ToggleAprilTagDetection(do_detection) => {
                    let mut tracker = shared_store_arc.write();
                    tracker.modify(|shared| {
                        if let Some(ref mut ts) = shared.apriltag_state {
                            ts.do_detection = do_detection;
                        } else {
                            error!("no apriltag support, not switching state");
                        }
                    });
                }
                CamArg::ToggleImOpsDetection(do_detection) => {
                    let mut tracker = shared_store_arc.write();
                    tracker.modify(|shared| {
                        shared.im_ops_state.do_detection = do_detection;
                    });
                }
                CamArg::SetImOpsDestination(v) => {
                    let mut tracker = shared_store_arc.write();
                    tracker.modify(|shared| {
                        shared.im_ops_state.destination = v;
                    });
                }
                CamArg::SetImOpsSource(v) => {
                    let mut tracker = shared_store_arc.write();
                    tracker.modify(|shared| {
                        shared.im_ops_state.source = v;
                    });
                }
                CamArg::SetImOpsCenterX(v) => {
                    let mut tracker = shared_store_arc.write();
                    tracker.modify(|shared| {
                        shared.im_ops_state.center_x = v;
                    });
                }
                CamArg::SetImOpsCenterY(v) => {
                    let mut tracker = shared_store_arc.write();
                    tracker.modify(|shared| {
                        shared.im_ops_state.center_y = v;
                    });
                }
                CamArg::SetImOpsThreshold(v) => {
                    let mut tracker = shared_store_arc.write();
                    tracker.modify(|shared| {
                        shared.im_ops_state.threshold = v;
                    });
                }
                CamArg::SetIsRecordingAprilTagCsv(do_recording) => {
                    let mut tracker = shared_store_arc.write();
                    tracker.modify(|shared| {
                        if let Some(ref mut ts) = shared.apriltag_state {

                            info!("changed recording april tag value: do_recording={}", do_recording);
                            let new_val = if do_recording {
                                tx_frame2.send(Msg::StartAprilTagRec(shared.format_str_apriltag_csv.clone())).cb_ok();
                                Some(RecordingPath::new(shared.format_str_apriltag_csv.clone()))
                            } else {
                                tx_frame2.send(Msg::StopAprilTagRec).cb_ok();
                                None
                            };
                            ts.is_recording_csv = new_val;

                        } else {
                            error!("no apriltag support, not switching state");
                        }

                    });
                }

                CamArg::PostTrigger(mkv_recording_config) => {
                    let format_str_mkv = {
                        let tracker = shared_store_arc.read();
                        tracker.as_ref().format_str_mkv.clone()
                    };
                    tx_frame2.send(Msg::PostTriggerStartMkv((format_str_mkv.clone(), mkv_recording_config))).cb_ok();
                    {
                        let mut tracker = shared_store_arc.write();
                        tracker.modify(|shared| {
                            shared.is_recording_mkv = Some(RecordingPath::new(format_str_mkv));
                        })
                    }
                }
                CamArg::SetPostTriggerBufferSize(size) => {
                    tx_frame2.send(Msg::SetPostTriggerBufferSize(size)).cb_ok();
                }
                CamArg::SetIsRecordingFmf(do_recording) => {
                    let mut tracker = shared_store_arc.write();
                    tracker.modify(|shared| {
                        let recording_framerate = shared.recording_framerate.clone();
                        if shared.is_recording_fmf.is_some() != do_recording {
                            info!("changed recording fmf value: do_recording={}", do_recording);
                            let new_val = if do_recording {
                                // change state
                                let local: chrono::DateTime<chrono::Local> = chrono::Local::now();
                                let filename = local.format(&shared.format_str).to_string();
                                tx_frame2.send(Msg::StartFMF((filename.clone(), recording_framerate))).cb_ok();
                                Some(RecordingPath::new(filename))
                            } else {
                                tx_frame2.send(Msg::StopFMF).cb_ok();
                                None
                            };
                            shared.is_recording_fmf = new_val;
                        }
                    });
                }
                CamArg::SetIsRecordingUfmf(do_recording) => {
                    #[cfg(feature="image_tracker")]
                    {
                        let mut tracker = shared_store_arc.write();
                        tracker.modify(|shared| {
                            if shared.is_recording_ufmf.is_some() != do_recording {
                                info!("changed recording ufmf value: do_recording={}", do_recording);
                                let new_val = if do_recording {
                                    if !shared.is_doing_object_detection {
                                        error!("Not doing object detection, ignoring command to save data to UFMF.");
                                        None
                                    } else {
                                        // change state
                                        let local: chrono::DateTime<chrono::Local> = chrono::Local::now();
                                        let filename = local.format(&shared.format_str_ufmf).to_string();
                                        tx_frame2.send(Msg::StartUFMF(filename.clone())).cb_ok();
                                        Some(RecordingPath::new(filename))
                                    }
                                } else {
                                    tx_frame2.send(Msg::StopUFMF).cb_ok();
                                    None
                                };
                                shared.is_recording_ufmf = new_val;
                            }
                        });
                    }
                }
                CamArg::SetIsDoingObjDetection(value) => {
                    #[cfg(feature="image_tracker")]
                    {
                        // update store
                        let mut tracker = shared_store_arc.write();
                        tracker.modify(|shared| {
                            shared.is_doing_object_detection = value;
                            tx_frame2
                                .send(Msg::SetTracking(value)).cb_ok();
                        });
                    }
                }
                CamArg::DoQuit => {
                    break;
                }
                CamArg::SetIsSavingObjDetectionCsv(value) => {
                    // update store in worker thread
                    #[cfg(feature="image_tracker")]
                    tx_frame2.send(Msg::SetIsSavingObjDetectionCsv(value)).cb_ok();
                }
                CamArg::SetObjDetectionConfig(yaml_buf) => {
                    // parse buffer
                    #[cfg(feature="image_tracker")]
                    match serde_yaml::from_str::<ImPtDetectCfg>(&yaml_buf) {
                        Err(e) => {error!("ignoring ImPtDetectCfg with parse error: {:?}", e)},
                        Ok(cfg) => {
                            let cfg2 = cfg.clone();

                            // Update config and send to frame process thread
                            let mut tracker = shared_store_arc.write();
                            tracker.modify(|shared| {

                                // Send config to frame process thread.
                                tx_frame2.send(Msg::SetExpConfig(cfg.clone())).cb_ok();
                                shared.im_pt_detect_cfg = cfg;
                            });

                            if let ImPtDetectCfgSource::ChangedSavedToDisk(ref src) = tracker_cfg_src {
                                let (ref app_info, ref prefs_key) = src;
                                match cfg2.save(app_info, prefs_key) {
                                    Ok(()) => {
                                        info!("saved new detection config");
                                    },
                                    Err(e) => {
                                        error!("saving preferences failed: \
                                            {} {:?}", e, e);
                                    }
                                }
                            }

                        }
                    }
                }
                CamArg::CamArgSetKalmanTrackingConfig(yaml_buf) => {
                    #[cfg(feature="flydratrax")]
                    {
                        // parse buffer
                        match serde_yaml::from_str::<KalmanTrackingConfig>(&yaml_buf) {
                            Err(e) => {error!("ignoring KalmanTrackingConfig with parse error: {:?}", e)},
                            Ok(cfg) => {
                                let cfg2 = cfg.clone();
                                {
                                    // Update config and send to frame process thread
                                    let mut tracker = shared_store_arc.write();
                                    tracker.modify(|shared| {
                                        shared.kalman_tracking_config = cfg;
                                    });
                                }
                                if let ImPtDetectCfgSource::ChangedSavedToDisk(ref src) = tracker_cfg_src {
                                    let (ref app_info, _) = src;
                                    match cfg2.save(app_info, KALMAN_TRACKING_PREFS_KEY) {
                                        Ok(()) => {
                                            info!("saved new kalman tracker config");
                                        }
                                        Err(e) => {
                                            error!("saving kalman tracker config failed: \
                                                {} {:?}", e, e);
                                        }
                                    }
                                } else {
                                    panic!("flydratrax requires saving changes to disk");
                                }
                            }
                        }
                    }
                }
                CamArg::CamArgSetLedProgramConfig(yaml_buf) => {
                    #[cfg(feature="flydratrax")]
                    {
                        // parse buffer
                        match serde_yaml::from_str::<LedProgramConfig>(&yaml_buf) {
                            Err(e) => {error!("ignoring LedProgramConfig with parse error: {:?}", e)},
                            Ok(cfg) => {
                                let cfg2 = cfg.clone();
                                {
                                    // Update config and send to frame process thread
                                    let mut tracker = shared_store_arc.write();
                                    tracker.modify(|shared| {
                                        shared.led_program_config = cfg;
                                    });
                                }
                                if let ImPtDetectCfgSource::ChangedSavedToDisk(ref src) = tracker_cfg_src {
                                    let (ref app_info, _) = src;
                                    match cfg2.save(app_info, LED_PROGRAM_PREFS_KEY) {
                                        Ok(()) => {
                                            info!("saved new LED program config");
                                        }
                                        Err(e) => {
                                            error!("saving LED program config failed: \
                                                {} {:?}", e, e);
                                        }
                                    }
                                } else {
                                    panic!("flydratrax requires saving changes to disk");
                                }
                            }
                        }
                    }
                }
                CamArg::ToggleCheckerboardDetection(val) => {
                    #[cfg(feature="checkercal")]
                    {
                        let mut tracker = shared_store_arc.write();
                        tracker.modify(|shared| {
                            shared.checkerboard_data.enabled = val;
                        });
                    }
                },
                CamArg::ToggleCheckerboardDebug(val) => {
                    #[cfg(feature="checkercal")]
                    {
                        let mut tracker = shared_store_arc.write();
                        tracker.modify(|shared| {
                            if val {
                                if shared.checkerboard_save_debug.is_none() {
                                    // start saving checkerboard data
                                    let basedir = std::env::temp_dir();

                                    let local: chrono::DateTime<chrono::Local> = chrono::Local::now();
                                    let format_str = "checkerboard_debug_%Y%m%d_%H%M%S";
                                    let stamped = local.format(&format_str).to_string();
                                    let dirname = basedir.join(stamped);
                                    info!("Saving checkerboard debug data to: {}", dirname.display());
                                    std::fs::create_dir_all(&dirname).unwrap();
                                    shared.checkerboard_save_debug = Some(format!("{}",dirname.display()));
                                }
                            } else {
                                if shared.checkerboard_save_debug.is_some() {
                                    // stop saving checkerboard data
                                    info!("Stop saving checkerboard debug data.");
                                    shared.checkerboard_save_debug = None;
                                }
                            }
                        });
                    }
                },

                CamArg::SetCheckerboardWidth(val) => {
                    #[cfg(feature="checkercal")]
                    {
                        let mut tracker = shared_store_arc.write();
                        tracker.modify(|shared| {
                            shared.checkerboard_data.width = val;
                        });
                    }
                },
                CamArg::SetCheckerboardHeight(val) => {
                    #[cfg(feature="checkercal")]
                    {
                        let mut tracker = shared_store_arc.write();
                        tracker.modify(|shared| {
                            shared.checkerboard_data.height = val;
                        });
                    }
                },
                CamArg::ClearCheckerboards => {
                    #[cfg(feature="checkercal")]
                    {
                        {
                            let mut collected_corners = collected_corners_arc.write();
                            collected_corners.clear();
                        }

                        {
                            let mut tracker = shared_store_arc.write();
                            tracker.modify(|shared| {
                                shared.checkerboard_data.num_checkerboards_collected = 0;
                            });
                        }

                    }
                },

                CamArg::PerformCheckerboardCalibration => {
                    #[cfg(feature="checkercal")]
                    {
                        info!("computing calibration");
                        let (n_rows, n_cols, checkerboard_save_debug) = {
                            let tracker = shared_store_arc.read();
                            let shared = (*tracker).as_ref();
                            let n_rows = shared.checkerboard_data.height;
                            let n_cols = shared.checkerboard_data.width;
                            let checkerboard_save_debug = shared.checkerboard_save_debug.clone();
                            (n_rows, n_cols, checkerboard_save_debug)
                        };

                        let goodcorners: Vec<camcal::CheckerBoardData> = {
                            let collected_corners = collected_corners_arc.read();
                            collected_corners.iter().map(|corners| {
                                let dim = 1.234; // TODO make this useful
                                let x: Vec<(f64,f64)> = corners.iter().map(|x| (x.0 as f64, x.1 as f64)).collect();
                                camcal::CheckerBoardData::new(dim, n_rows as usize, n_cols as usize, &x)
                            }).collect()
                        };

                        let ros_cam_name = cam_name2.to_ros();
                        let local: chrono::DateTime<chrono::Local> = chrono::Local::now();

                        if let Some(debug_dir) = &checkerboard_save_debug {
                            let format_str = format!("checkerboard_input_{}.%Y%m%d_%H%M%S.yaml", ros_cam_name.as_str());
                            let stamped = local.format(&format_str).to_string();

                            let debug_path = std::path::PathBuf::from(debug_dir);
                            let corners_path = debug_path.join(stamped);

                            let f = File::create(
                                &corners_path)
                                .expect("create file");

                            #[derive(Serialize)]
                            struct CornersData<'a> {
                                corners: &'a Vec<camcal::CheckerBoardData>,
                                image_width: u32,
                                image_height: u32,
                            }
                            let debug_data = CornersData {
                                corners: &goodcorners,
                                image_width,
                                image_height,
                            };
                            serde_yaml::to_writer(f, &debug_data)
                                .expect("serde_yaml::to_writer");
                        }

                        let size = camcal::PixelSize::new(image_width as usize,image_height as usize);
                        match camcal::compute_intrinsics::<f64>(size, &goodcorners) {
                            Ok(intrinsics) => {
                                info!("got calibrated intrinsics: {:?}", intrinsics);

                                // Convert from mvg to ROS format.
                                let ci: opencv_ros_camera::RosCameraInfo<_> = opencv_ros_camera::NamedIntrinsicParameters {
                                    intrinsics,
                                    width: image_width as usize,
                                    height: image_height as usize,
                                    name: ros_cam_name.as_str().to_string(),
                                }.into();

                                let cal_dir = app_dirs::app_dir(
                                    app_dirs::AppDataType::UserConfig,
                                    &APP_INFO, "camera_info").expect("app_dirs::app_dir");

                                let format_str = format!("{}.%Y%m%d_%H%M%S.yaml", ros_cam_name.as_str());
                                let stamped = local.format(&format_str).to_string();
                                let cam_info_file_stamped = cal_dir.join(stamped);

                                let mut cam_info_file = cal_dir.clone();
                                cam_info_file.push(ros_cam_name.as_str());
                                cam_info_file.set_extension("yaml");

                                // Save timestamped version first for backup
                                // purposes (since below we overwrite the
                                // non-timestamped file).
                                {
                                    let f = File::create(
                                        &cam_info_file_stamped)
                                        .expect("create file");
                                    serde_yaml::to_writer(f, &ci)
                                    .expect("serde_yaml::to_writer");
                                }

                                // Now copy the successfully saved file into
                                // the non-timestamped name. This will
                                // overwrite an existing file.
                                std::fs::copy(
                                    &cam_info_file_stamped,
                                    &cam_info_file)
                                    .expect("copy file");

                                info!("Saved camera calibration to file: {}",
                                    cam_info_file.display());

                            },
                            Err(e) => {
                                error!("failed doing calibration {:?} {}", e, e);
                            }
                        };
                    }
                },
            }

        }

        // We get here iff DoQuit broke us out of infinite loop.

        // In theory, all things currently being saved should nicely stop themselves when dropped.
        // For now, while we are working on ctrlc handling, we manually stop them.
        tx_frame2.send(Msg::StopFMF).cb_ok();
        tx_frame2.send(Msg::StopMkv).cb_ok();
        #[cfg(feature="image_tracker")]
        tx_frame2.send(Msg::StopUFMF).cb_ok();
        #[cfg(feature="image_tracker")]
        tx_frame2.send(Msg::SetIsSavingObjDetectionCsv(CsvSaveConfig::NotSaving)).cb_ok();

        tx_frame2.send(Msg::QuitFrameProcessThread).cb_ok(); // this will quit the frame_process_thread

        // Tell all streams to quit.
        debug!("*** sending quit trigger to all valved streams. **** {}:{}", file!(), line!());
        quit_trigger.cancel();
        debug!("*** sending shutdown to hyper **** {}:{}", file!(), line!());
        shutdown_tx.send(()).expect("sending shutdown to hyper");

        #[cfg(feature="flydratrax")]
        model_server_shutdown_tx.send(()).expect("sending shutdown to model server");

        #[cfg(feature="debug-images")]
        debug_image_shutdown_tx.send(()).expect("sending shutdown to model server");

        info!("attempting to nicely stop camera");
        if let Some((control, join_handle)) = cam.control_and_join_handle() {
            control.stop();
            while !control.is_done() {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            info!("camera thread stopped");
            join_handle.join().expect("join camera thread");
            info!("camera thread joined");
        } else {
            error!("camera thread not running!?");
        }

        info!("cam_args_rx future is resolved");
    }};

    if !args.no_browser {
        // sleep to let the webserver start before opening browser
        std::thread::sleep(std::time::Duration::from_millis(100));

        open_browser(url.clone())?;
    } else {
        info!("listening at {}", url);
    }

    let sender_table = my_app.txers.clone();

    let (flag, control) = thread_control::make_pair();
    let cam_args_tx3 = cam_args_tx2.clone();
    let join_handle = std::thread::Builder::new().name("video_streaming".to_string()).spawn(move || { // confirmed closes
        let thread_closer = CloseAppOnThreadExit::new(cam_args_tx3, file!(), line!());
        thread_closer.maybe_err(video_streaming::firehose_thread(
            sender_table,
            firehose_rx,
            firehose_callback_rx,
            false,
            &strand_cam_storetype::STRAND_CAM_EVENTS_URL_PATH,
            flag,
        ).map_err(|e| anyhow::Error::from(e)));
    })?.into();
    let video_streaming_cjh = ControlledJoinHandle { control, join_handle };

    #[cfg(feature="plugin-process-frame")]
    let plugin_streaming_cjh = {
        let (flag, control) = thread_control::make_pair();
        let join_handle = std::thread::Builder::new().name("plugin_streaming".to_string()).spawn(move || { // ignore plugin
            let thread_closer = CloseAppOnThreadExit::new(cam_args_tx2, file!(), line!());
            while flag.is_alive() {
                let frame = thread_closer.check(plugin_handler_thread_rx.recv());
                if let Some(ref pfc) = process_frame_callback {
                    let c_data = view_as_c_frame(&frame);
                    let c_timestamp = get_c_timestamp(&frame);
                    let ffi_result = (pfc.func_ptr)(&c_data, pfc.data_handle, c_timestamp);
                    let points = ffi_to_points(&ffi_result);
                    thread_closer.check(plugin_result_tx.send(points));
                }
            }
            thread_closer.success();
        })?.into();
        ControlledJoinHandle { control, join_handle }
    };

    debug!("  running forever");

    // In the case of #[cfg(feature="with-camtrig")], this will spawn
    // the serial thread that communicates with the camtrig device.
    // Otherwise, does very little and `sjh` is essentially empty.
    let sjh = my_app.maybe_spawn_camtrig_thread(camtrig_tx_std, camtrig_rx,
        camtrig_heartbeat_update_arc, cam_args_tx.clone())?;

    let ajh = AllJoinHandles {
        sjh,
        frame_process_cjh,
        video_streaming_cjh,
        #[cfg(feature="plugin-process-frame")]
        plugin_streaming_cjh,
    };

    let cam_arg_future2 = async move {
        cam_arg_future.await;

        // we get here once the whole program is trying to shut down.
        let stoppers = ajh.stoppers();
        for stopper in stoppers.iter() {
            debug!("sending stop message to thread {}:{}", file!(), line!());
            stopper.stop();
        }

        info!("reactor shutdown, now stopping spawned threads");
        ajh.close_and_join_all().expect("failed closing and joining threads");

    };

    Ok((http_camserver_info, cam_args_tx, cam_arg_future2, my_app))
}

pub struct ControlledJoinHandle<T> {
    control: thread_control::Control,
    join_handle: std::thread::JoinHandle<T>,
}

impl<T> ControlledJoinHandle<T> {
    fn close_and_join(self) -> std::thread::Result<T> {
        debug!(
            "sending stop {:?} {:?}",
            self.join_handle.thread().name(),
            self.join_handle.thread().id()
        );
        self.control.stop();
        while !self.control.is_done() {
            debug!(
                "waiting for stop {:?} {:?}",
                self.join_handle.thread().name(),
                self.join_handle.thread().id()
            );
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        debug!(
            "joining {:?} {:?}",
            self.join_handle.thread().name(),
            self.join_handle.thread().id()
        );
        let result = self.join_handle.join();
        debug!("joining done");
        result
    }
}

pub struct AllJoinHandles {
    sjh: SerialJoinHandles,
    frame_process_cjh: ControlledJoinHandle<()>,
    video_streaming_cjh: ControlledJoinHandle<()>,
    #[cfg(feature = "plugin-process-frame")]
    plugin_streaming_cjh: ControlledJoinHandle<()>,
}

impl AllJoinHandles {
    fn close_and_join_all(self) -> std::thread::Result<()> {
        self.sjh.close_and_join_all()?;
        self.frame_process_cjh.close_and_join()?;
        self.video_streaming_cjh.close_and_join()?;
        #[cfg(feature = "plugin-process-frame")]
        self.plugin_streaming_cjh.close_and_join()?;
        Ok(())
    }
    fn stoppers(&self) -> Vec<thread_control::Control> {
        let mut result = vec![
            self.frame_process_cjh.control.clone(),
            self.video_streaming_cjh.control.clone(),
            #[cfg(feature = "plugin-process-frame")]
            self.plugin_streaming_cjh.control.clone(),
        ];
        result.extend(self.sjh.stoppers().into_iter());
        result
    }
}

#[cfg(feature = "plugin-process-frame")]
fn ffi_to_points(
    pts: &plugin_defs::StrandCamFrameAnnotation,
) -> Vec<http_video_streaming_types::Point> {
    pts.as_slice()
        .iter()
        .map(|pt| http_video_streaming_types::Point {
            x: pt.x,
            y: pt.y,
            area: None,
            theta: None,
        })
        .collect()
}

#[cfg(feature = "plugin-process-frame")]
fn get_pixfmt(pixfmt: &PixFmt) -> plugin_defs::EisvogelPixelFormat {
    match pixfmt {
        PixFmt::Mono8 => plugin_defs::EisvogelPixelFormat::MONO8,
        PixFmt::BayerRG8 => plugin_defs::EisvogelPixelFormat::BayerRG8,
        other => panic!("unsupported pixel format: {}", other),
    }
}

#[cfg(feature = "plugin-process-frame")]
fn get_c_timestamp<'a>(frame: &'a DynamicFrame) -> f64 {
    let ts = frame.extra().host_timestamp();
    datetime_conversion::datetime_to_f64(&ts)
}

#[cfg(feature = "plugin-process-frame")]
fn view_as_c_frame<'a>(frame: &'a DynamicFrame) -> plugin_defs::FrameData {
    use formats::Stride;

    let pixel_format = get_pixfmt(&frame.pixel_format());

    let result = plugin_defs::FrameData {
        data: frame.image_data_without_format().as_ptr() as *const i8,
        stride: frame.stride() as u64,
        rows: frame.height(),
        cols: frame.width(),
        pixel_format,
    };
    result
}

fn open_browser(url: String) -> Result<()> {
    // Spawn a new thread because xdg-open blocks forever
    // if it must open a new browser.
    std::thread::Builder::new()
        .name("browser opener".to_string())
        .spawn(move || {
            // ignore browser
            info!("Opening browser at {}", url);
            match webbrowser::open(&url) {
                Ok(_) => trace!("Browser opened"),
                Err(e) => error!("Error opening brower: {:?}", e),
            };
            debug!("browser thread done {}:{}", file!(), line!());
        })?;
    Ok(())
}

#[cfg(feature = "fiducial")]
fn make_family(family: &ci2_remote_control::TagFamily) -> apriltag::Family {
    use ci2_remote_control::TagFamily::*;
    match family {
        Family36h11 => apriltag::Family::new_tag_36h11(),
        FamilyStandard41h12 => apriltag::Family::new_tag_standard_41h12(),
        Family16h5 => apriltag::Family::new_tag_16h5(),
        Family25h9 => apriltag::Family::new_tag_25h9(),
        FamilyCircle21h7 => apriltag::Family::new_tag_circle_21h7(),
        FamilyCircle49h12 => apriltag::Family::new_tag_circle_49h12(),
        FamilyCustom48h12 => apriltag::Family::new_tag_custom_48h12(),
        FamilyStandard52h13 => apriltag::Family::new_tag_standard_52h13(),
    }
}
